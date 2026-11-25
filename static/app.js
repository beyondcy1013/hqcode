import {
  I18N,
  MOBILE_KEY_MAP,
  detectDefaultLang,
  parseUnifiedDiffLines,
} from '/static/hq-helpers.js';

const state = {
  lang: localStorage.getItem('hqcode_lang') || detectDefaultLang(navigator.language),
  sessions: [],
  activeSessionId: null,
  currentPath: '',
  editingFile: null,
  selectedDiffFile: null,
  ws: null,
  term: null,
  fitAddon: null,
  userScrolledUp: false,
  renamingId: null,
  pollTimer: null,
  qrExpiresAt: 0,
  qrTimer: null,
  presetsCache: [],
};

const $ = (id) => document.getElementById(id);

function t(key) {
  return (I18N[state.lang] && I18N[state.lang][key]) || I18N.en[key] || key;
}

function applyI18n() {
  document.querySelectorAll('[data-i18n]').forEach((el) => {
    el.textContent = t(el.dataset.i18n);
  });
  document.querySelectorAll('[data-i18n-placeholder]').forEach((el) => {
    el.placeholder = t(el.dataset.i18nPlaceholder);
  });
  const toggleText = state.lang === 'zh' ? '中文' : 'EN';
  if ($('btn-lang-toggle')) $('btn-lang-toggle').textContent = toggleText;
  if ($('btn-lang-login')) $('btn-lang-login').textContent = toggleText;
  updatePresetSubmitButton();
  updateLaunchCwdPill();
}

function toggleLang() {
  state.lang = state.lang === 'en' ? 'zh' : 'en';
  localStorage.setItem('hqcode_lang', state.lang);
  applyI18n();
  renderSessions();
  refreshGitStatus();
  if (!$('connect-modal').classList.contains('hidden')) {
    refreshConnectModal(false);
  }
  if (!$('settings-modal').classList.contains('hidden')) {
    loadSettingsData();
  }
}

function showToast(message, kind = 'info') {
  const host = $('toast-container');
  const el = document.createElement('div');
  el.className = `toast ${kind}`;
  el.textContent = message;
  host.appendChild(el);
  setTimeout(() => el.remove(), 3200);
}

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

function attachConfirmAction(btn, idleLabel, confirmLabel, onConfirmed) {
  let armed = false;
  let armTimer = null;
  btn.textContent = idleLabel;
  btn.addEventListener('click', async (ev) => {
    ev.stopPropagation();
    if (!armed) {
      armed = true;
      btn.textContent = confirmLabel;
      btn.classList.add('confirming');
      armTimer = setTimeout(() => {
        armed = false;
        btn.textContent = idleLabel;
        btn.classList.remove('confirming');
      }, 3000);
      return;
    }
    clearTimeout(armTimer);
    armed = false;
    btn.textContent = idleLabel;
    btn.classList.remove('confirming');
    await onConfirmed();
  });
}

function inferFriendlyDeviceName() {
  const ua = navigator.userAgent || '';
  let os = 'Browser';
  if (/iphone/i.test(ua)) os = 'iPhone';
  else if (/ipad/i.test(ua)) os = 'iPad';
  else if (/android/i.test(ua)) os = 'Android';
  else if (/mac os/i.test(ua)) os = 'macOS';
  else if (/windows/i.test(ua)) os = 'Windows';
  else if (/linux/i.test(ua)) os = 'Linux';

  let browser = '';
  if (/edg\//i.test(ua)) browser = 'Edge';
  else if (/chrome/i.test(ua)) browser = 'Chrome';
  else if (/safari/i.test(ua)) browser = 'Safari';
  else if (/firefox/i.test(ua)) browser = 'Firefox';

  return browser ? `${os} · ${browser}` : os;
}

function syncVisualViewportHeight() {
  const app = $('app-view');
  if (!app || !window.visualViewport) return;
  app.style.height = `${window.visualViewport.height}px`;
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

document.addEventListener('keydown', (e) => {
  if ((e.ctrlKey || e.metaKey) && e.key.toLowerCase() === 'k') {
    e.preventDefault();
    $('session-search-input')?.focus();
  } else if ((e.ctrlKey || e.metaKey) && e.shiftKey && e.key.toLowerCase() === 'd') {
    e.preventDefault();
    $('btn-open-diff')?.click();
  }
});

async function tryRedeemPairCodeFromUrl() {
  const params = new URLSearchParams(location.search);
  const code = params.get('code');
  if (!code) return null;

  history.replaceState({}, '', '/');
  const banner = $('pairing-banner');
  $('login-view').classList.remove('hidden');
  banner.className = 'pairing-banner';
  banner.textContent = t('pairingInProgress');

  try {
    await api('/api/remote/redeem', {
      method: 'POST',
      body: JSON.stringify({
        code,
        device_name: inferFriendlyDeviceName(),
      }),
    });
    banner.classList.add('hidden');
    return { paired: true };
  } catch (err) {
    banner.className = 'pairing-banner error';
    banner.textContent = `${t('pairingFailedPrefix')}: ${err.message}`;
    return { paired: false };
  }
}

async function bootstrap() {
  applyI18n();
  const pairOutcome = await tryRedeemPairCodeFromUrl();
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
  await Promise.all([
    loadDetectedAgents(),
    loadSessions(),
    loadFiles(''),
    refreshGitStatus(),
    refreshSystemInfo(),
  ]);
  if (pairOutcome?.paired) {
    showToast(t('pairedWelcome'), 'ok');
  }
  if (!state.pollTimer) {
    state.pollTimer = setInterval(() => {
      loadSessions();
      refreshSystemInfo();
    }, 5000);
  }
}

$('btn-lang-toggle').addEventListener('click', toggleLang);
$('btn-lang-login').addEventListener('click', toggleLang);

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

async function refreshSystemInfo() {
  try {
    const info = await api('/api/system/info');
    const pill = $('rss-pill');
    if (info.rss_kb) {
      const mb = (info.rss_kb / 1024).toFixed(1);
      pill.textContent = `RSS ${mb} MB`;
      pill.classList.remove('hidden');
    }
  } catch (_) {}
}

function updateLaunchCwdPill() {
  const pill = $('launch-cwd-pill');
  if (!pill) return;
  if (state.currentPath) {
    pill.textContent = `📁 ${state.currentPath} ✕`;
    pill.title = state.currentPath;
    pill.classList.remove('hidden');
  } else {
    pill.textContent = '';
    pill.classList.add('hidden');
  }
}

$('launch-cwd-pill')?.addEventListener('click', () => {
  loadFiles('');
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
      const suffix = state.currentPath ? ` (${state.currentPath.split('/').pop()})` : '';
      createSession(`${agent.label}${suffix}`, cmd, state.currentPath || null);
    });
    host.appendChild(btn);
  }
}

async function loadSessions() {
  try {
    const data = await api('/api/terminal/sessions');
    state.sessions = data.sessions || [];
    renderSessions();
    if (!state.activeSessionId && state.sessions.length > 0) {
      selectSession(state.sessions[0].id);
    }
  } catch (_) {}
}

function statusDotClass(s) {
  if (!s.alive || s.status === 'exited') return 'dot-exited';
  return s.status === 'active' ? 'dot-active' : 'dot-idle';
}

function statusDotChar(s) {
  return s.alive ? '●' : '○';
}

function startInlineRename(session, container) {
  state.renamingId = session.id;
  container.innerHTML = '';
  const input = document.createElement('input');
  input.type = 'text';
  input.className = 'inline-rename-input';
  input.value = session.name;
  let committed = false;

  const finish = async (save) => {
    if (committed) return;
    committed = true;
    state.renamingId = null;
    const next = input.value.trim();
    if (save && next && next !== session.name) {
      try {
        await api(`/api/terminal/sessions/${session.id}`, {
          method: 'PUT',
          body: JSON.stringify({ name: next }),
        });
      } catch (err) {
        showToast(err.message, 'err');
      }
    }
    await loadSessions();
  };

  input.addEventListener('keydown', (ev) => {
    if (ev.key === 'Enter') {
      ev.preventDefault();
      finish(true);
    } else if (ev.key === 'Escape') {
      ev.preventDefault();
      finish(false);
    }
  });
  input.addEventListener('blur', () => finish(true));
  container.appendChild(input);
  input.focus();
  input.select();
}

function renderMobileSessionSelect() {
  const select = $('mobile-session-select');
  if (!select) return;
  select.innerHTML = '';
  if (state.sessions.length === 0) {
    const opt = document.createElement('option');
    opt.value = '';
    opt.textContent = t('noSessionsOption');
    select.appendChild(opt);
    select.disabled = true;
    return;
  }
  select.disabled = false;
  for (const s of state.sessions) {
    const opt = document.createElement('option');
    opt.value = s.id;
    opt.textContent = `${statusDotChar(s)} ${s.name}`;
    opt.selected = s.id === state.activeSessionId;
    select.appendChild(opt);
  }
}

$('mobile-session-select')?.addEventListener('change', (ev) => {
  const sid = ev.target.value;
  if (sid) {
    selectSession(sid);
  }
});

function renderSessions() {
  const list = $('session-list');
  const tabs = $('session-tabs');
  list.innerHTML = '';
  tabs.innerHTML = '';
  renderMobileSessionSelect();

  for (const s of state.sessions) {
    // Sidebar item
    const row = document.createElement('div');
    row.className = `session-item${s.id === state.activeSessionId ? ' active' : ''}`;
    row.dataset.sessionId = s.id;

    if (state.renamingId === s.id) {
      startInlineRename(s, row);
    } else {
      const title = document.createElement('span');
      title.className = 'session-title';
      const dot = document.createElement('span');
      dot.className = statusDotClass(s);
      dot.textContent = `${statusDotChar(s)} `;
      title.append(dot, document.createTextNode(s.name));
      title.addEventListener('click', () => {
        selectSession(s.id);
        $('sidebar').classList.remove('open');
      });

      const renameBtn = document.createElement('button');
      renameBtn.className = 'mini-btn';
      renameBtn.textContent = '✎';
      renameBtn.addEventListener('click', (ev) => {
        ev.stopPropagation();
        startInlineRename(s, row);
      });

      const closeBtn = document.createElement('button');
      closeBtn.className = 'mini-btn session-close-btn';
      attachConfirmAction(closeBtn, '✕', t('confirmClose'), async () => {
        await closeSession(s.id);
      });

      row.append(title, renameBtn, closeBtn);
    }
    list.appendChild(row);

    // Top tab item
    const tab = document.createElement('div');
    tab.className = `session-tab${s.id === state.activeSessionId ? ' active' : ''}`;
    tab.dataset.sessionId = s.id;
    const tDot = document.createElement('span');
    tDot.className = statusDotClass(s);
    tDot.textContent = statusDotChar(s);
    const tLabel = document.createElement('span');
    tLabel.textContent = s.name;
    tLabel.addEventListener('dblclick', (ev) => {
      ev.stopPropagation();
      startInlineRename(s, row);
    });
    const tClose = document.createElement('button');
    tClose.type = 'button';
    tClose.className = 'tab-close-btn mini-btn';
    attachConfirmAction(tClose, '×', t('confirmClose'), async () => {
      await closeSession(s.id);
    });
    tab.append(tDot, tLabel, tClose);
    tab.addEventListener('click', () => selectSession(s.id));
    tabs.appendChild(tab);
  }
}

async function closeSession(id) {
  await api(`/api/terminal/sessions/${id}`, { method: 'DELETE' });
  if (state.activeSessionId === id) {
    state.activeSessionId = null;
    disconnectTerminal();
  }
  await loadSessions();
  refreshGitStatus();
}

async function createSession(label, startupCommand, cwd = null) {
  try {
    const res = await api('/api/terminal/sessions', {
      method: 'POST',
      body: JSON.stringify({
        cwd,
        name: label ? `${label} ${state.sessions.length + 1}` : null,
        startup_command: startupCommand || null,
      }),
    });
    if (res.session) {
      state.activeSessionId = res.session.id;
    }
    await loadSessions();
    if (res.session) {
      selectSession(res.session.id, true);
      $('sidebar').classList.remove('open');
    }
  } catch (err) {
    showToast(err.message, 'err');
  }
}

$('btn-new-shell').addEventListener('click', () => {
  const baseName = state.currentPath ? `Shell (${state.currentPath.split('/').pop()})` : 'Shell';
  createSession(baseName, '', state.currentPath || null);
});

// Session output search
let searchTimer = null;
$('session-search-input').addEventListener('input', (e) => {
  clearTimeout(searchTimer);
  const q = e.target.value.trim();
  const box = $('search-results');
  if (!q) {
    box.classList.add('hidden');
    box.innerHTML = '';
    return;
  }
  searchTimer = setTimeout(async () => {
    try {
      const data = await api(`/api/terminal/sessions/search?q=${encodeURIComponent(q)}`);
      const matches = data.matches || [];
      box.innerHTML = '';
      if (matches.length === 0) {
        box.classList.add('hidden');
        return;
      }
      for (const m of matches.slice(0, 12)) {
        const hit = document.createElement('div');
        hit.className = 'search-hit';
        const name = document.createElement('div');
        name.className = 'search-hit-name';
        name.textContent = m.session_name;
        const line = document.createElement('div');
        line.className = 'search-hit-line';
        line.textContent = m.line;
        hit.append(name, line);
        hit.addEventListener('click', () => {
          box.classList.add('hidden');
          selectSession(m.session_id);
        });
        box.appendChild(hit);
      }
      box.classList.remove('hidden');
    } catch (_) {}
  }, 180);
});

document.addEventListener('click', (e) => {
  if (!e.target.closest('.search-wrap')) {
    $('search-results').classList.add('hidden');
  }
});

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
    $('btn-jump-bottom').classList.toggle('hidden', !state.userScrolledUp);
  });

  term.onData((data) => {
    if (state.ws && state.ws.readyState === WebSocket.OPEN) {
      state.ws.send(JSON.stringify({ type: 'input', data }));
    }
  });

  state.term = term;
  state.fitAddon = fitAddon;
  window.__hqTerm = term;
}

$('btn-jump-bottom').addEventListener('click', () => {
  if (state.term) {
    state.term.scrollToBottom();
    state.userScrolledUp = false;
    $('btn-jump-bottom').classList.add('hidden');
    state.term.focus();
  }
});

function setConnPill(mode) {
  const pill = $('conn-pill');
  if (!mode) {
    pill.classList.add('hidden');
    return;
  }
  pill.classList.remove('hidden', 'live', 'reconnecting');
  pill.classList.add(mode);
  pill.textContent = mode === 'live' ? t('live') : t('reconnecting');
}

function disconnectTerminal() {
  if (state.ws) {
    state.ws.close();
    state.ws = null;
  }
  setConnPill(null);
  $('xterm-host').classList.add('hidden');
  $('empty-state').classList.remove('hidden');
  $('btn-jump-bottom').classList.add('hidden');
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

function selectSession(sessionId, forceReconnect = false) {
  if (
    !forceReconnect &&
    sessionId === state.activeSessionId &&
    state.ws &&
    state.ws.readyState === WebSocket.OPEN
  ) {
    state.term?.focus();
    renderMobileSessionSelect();
    return;
  }

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
  $('btn-jump-bottom').classList.add('hidden');
  state.fitAddon.fit();

  const proto = location.protocol === 'https:' ? 'wss' : 'ws';
  const ws = new WebSocket(`${proto}://${location.host}/ws/terminal/${sessionId}`);
  ws.binaryType = 'arraybuffer';

  ws.onopen = () => {
    setConnPill('live');
    sendResize();
  };
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
      setConnPill('reconnecting');
      setTimeout(() => {
        if (state.activeSessionId === sessionId && (!state.ws || state.ws.readyState === WebSocket.CLOSED)) {
          selectSession(sessionId, true);
        }
      }, 1500);
    }
  };

  state.ws = ws;
  state.term.focus();
  refreshGitStatus();
}

async function loadFiles(path) {
  state.currentPath = path;
  updateLaunchCwdPill();
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
    const label = document.createElement('span');
    label.className = 'file-name';
    label.textContent = `${entry.kind === 'dir' ? '📁' : '📄'} ${entry.name}`;
    label.addEventListener('click', () => {
      if (entry.kind === 'dir') {
        loadFiles(entry.path);
      } else {
        openFile(entry.path);
      }
    });
    row.appendChild(label);

    if (entry.kind === 'dir' && entry.name !== '.git') {
      const termBtn = document.createElement('button');
      termBtn.className = 'mini-btn dir-term-btn';
      termBtn.textContent = '>_';
      termBtn.title = t('terminalHere');
      termBtn.addEventListener('click', (ev) => {
        ev.stopPropagation();
        createSession(entry.name, '', entry.path);
      });
      row.appendChild(termBtn);
    }

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
  try {
    await api('/api/file', {
      method: 'PUT',
      body: JSON.stringify({
        path: state.editingFile,
        content: $('editor-textarea').value,
      }),
    });
    $('editor-overlay').classList.add('hidden');
    showToast(t('fileSaved'), 'ok');
    refreshGitStatus();
  } catch (err) {
    showToast(err.message, 'err');
  }
});

$('btn-close-editor').addEventListener('click', () => $('editor-overlay').classList.add('hidden'));

function gitSessionQuery() {
  return state.activeSessionId ? `session_id=${encodeURIComponent(state.activeSessionId)}` : '';
}

async function refreshGitStatus() {
  try {
    const q = gitSessionQuery();
    const status = await api(`/api/git/status${q ? `?${q}` : ''}`);
    const badge = $('diff-count-badge');
    if (!status.is_repo || status.files.length === 0) {
      badge.classList.add('hidden');
    } else {
      badge.textContent = String(status.files.length);
      badge.classList.remove('hidden');
    }
    $('diff-branch-pill').textContent = status.is_repo ? status.branch : 'no-git';
    const statPill = $('diff-stat-pill');
    if (status.is_repo && (status.insertions > 0 || status.deletions > 0)) {
      statPill.innerHTML = `<span class="stat-add">+${status.insertions}</span> / <span class="stat-del">-${status.deletions}</span>`;
      statPill.classList.remove('hidden');
    } else {
      statPill.classList.add('hidden');
    }
    renderDiffFileList(status.files || []);
  } catch (_) {}
}

function renderDiffFileList(files) {
  const list = $('diff-file-list');
  list.innerHTML = '';
  if (files.length === 0) {
    const empty = document.createElement('div');
    empty.style.color = '#8b949e';
    empty.style.padding = '6px';
    empty.textContent = t('workingTreeClean');
    list.appendChild(empty);
    $('diff-viewer').textContent = '';
    return;
  }
  for (const f of files) {
    const row = document.createElement('div');
    row.className = `diff-file-row${state.selectedDiffFile === f.path ? ' active' : ''}`;

    const label = document.createElement('span');
    label.className = 'file-name mono';
    const stat = (f.insertions || f.deletions) ? ` (+${f.insertions}/-${f.deletions})` : '';
    label.textContent = `[${f.status}] ${f.path}${stat}`;
    label.addEventListener('click', () => loadDiff(f.path));

    const revertBtn = document.createElement('button');
    revertBtn.className = 'mini-btn';
    attachConfirmAction(revertBtn, t('revert'), t('confirmRevert'), async () => {
      try {
        await api('/api/git/revert', {
          method: 'POST',
          body: JSON.stringify({ file: f.path, session_id: state.activeSessionId }),
        });
        showToast(`${t('revertedPrefix')}: ${f.path}`, 'ok');
        await refreshGitStatus();
        await loadDiff(null);
      } catch (err) {
        showToast(err.message, 'err');
      }
    });

    row.append(label, revertBtn);
    list.appendChild(row);
  }
}

async function loadDiff(file) {
  state.selectedDiffFile = file;
  const params = new URLSearchParams();
  if (file) params.set('file', file);
  if (state.activeSessionId) params.set('session_id', state.activeSessionId);
  const qs = params.toString();
  const data = await api(`/api/git/diff${qs ? `?${qs}` : ''}`);
  renderColorDiff(data.diff || '');
}

function renderColorDiff(diffText) {
  const viewer = $('diff-viewer');
  viewer.innerHTML = '';
  if (!diffText.trim()) {
    const empty = document.createElement('div');
    empty.style.padding = '12px';
    empty.style.color = '#8b949e';
    empty.textContent = t('noDiff');
    viewer.appendChild(empty);
    return;
  }
  const rows = parseUnifiedDiffLines(diffText);
  const frag = document.createDocumentFragment();
  for (const r of rows) {
    const row = document.createElement('div');
    row.className = `diff-row ${r.kind}`;
    const o = document.createElement('span');
    o.className = 'diff-ln';
    o.textContent = r.oldNum;
    const n = document.createElement('span');
    n.className = 'diff-ln';
    n.textContent = r.newNum;
    const code = document.createElement('span');
    code.className = 'diff-code';
    code.textContent = r.text || ' ';
    row.append(o, n, code);
    frag.appendChild(row);
  }
  viewer.appendChild(frag);
}

$('btn-open-diff').addEventListener('click', async () => {
  $('diff-drawer').classList.toggle('hidden');
  if (!$('diff-drawer').classList.contains('hidden')) {
    await refreshGitStatus();
    await loadDiff(state.selectedDiffFile);
  }
});
$('btn-diff-all').addEventListener('click', () => loadDiff(null));
$('btn-close-diff').addEventListener('click', () => $('diff-drawer').classList.add('hidden'));
$('btn-refresh-diff').addEventListener('click', async () => {
  await refreshGitStatus();
  await loadDiff(state.selectedDiffFile);
});

$('btn-git-commit').addEventListener('click', async () => {
  const message = $('commit-message-input').value.trim();
  if (!message) return;
  try {
    const res = await api('/api/git/commit', {
      method: 'POST',
      body: JSON.stringify({ message, session_id: state.activeSessionId }),
    });
    $('commit-message-input').value = '';
    showToast(`${t('committedPrefix')}: ${res.commit}`, 'ok');
    await refreshGitStatus();
    await loadDiff(null);
  } catch (err) {
    showToast(err.message, 'err');
  }
});

$('btn-toggle-sidebar').addEventListener('click', () => {
  $('sidebar').classList.toggle('open');
});

document.querySelectorAll('#hq-mobile-keys .mkey').forEach((btn) => {
  btn.addEventListener('click', async () => {
    if (btn.dataset.action === 'diff') {
      $('btn-open-diff').click();
      return;
    }
    if (!state.activeSessionId) return;
    if (btn.dataset.key === 'enter') {
      await api(`/api/terminal/sessions/${state.activeSessionId}/enter`, { method: 'POST' });
      return;
    }
    const seq = MOBILE_KEY_MAP[btn.dataset.key];
    if (seq && state.ws && state.ws.readyState === WebSocket.OPEN) {
      state.ws.send(JSON.stringify({ type: 'input', data: seq }));
    }
  });
});

async function sendMobileQuickInput() {
  const input = $('mobile-quick-input');
  const val = input.value;
  if (!val || !state.activeSessionId) return;
  await api(`/api/terminal/sessions/${state.activeSessionId}/input`, {
    method: 'POST',
    body: JSON.stringify({ data: `${val}\r` }),
  });
  input.value = '';
}

$('btn-mobile-send').addEventListener('click', sendMobileQuickInput);
$('mobile-quick-input').addEventListener('keydown', (e) => {
  if (e.key === 'Enter' && !e.isComposing) {
    e.preventDefault();
    sendMobileQuickInput();
  }
});

// Remote QR Connect modal
function renderSvgInto(host, svgMarkup) {
  host.innerHTML = '';
  if (!svgMarkup) return;
  const doc = new DOMParser().parseFromString(svgMarkup, 'image/svg+xml');
  const svg = doc.documentElement;
  if (svg && svg.nodeName.toLowerCase() === 'svg') {
    host.appendChild(document.importNode(svg, true));
  }
}

function startQrCountdown(expiresInSecs) {
  if (state.qrTimer) {
    clearInterval(state.qrTimer);
    state.qrTimer = null;
  }
  state.qrExpiresAt = Date.now() + expiresInSecs * 1000;
  const update = () => {
    const rem = Math.max(0, Math.floor((state.qrExpiresAt - Date.now()) / 1000));
    const el = $('connect-qr-ttl');
    if (!el) return;
    el.textContent = rem > 0 ? `${t('expiresInPrefix')}: ${rem}s` : t('qrExpired');
  };
  update();
  state.qrTimer = setInterval(update, 1000);
}

function renderPairedDevices(devices) {
  const host = $('paired-devices-list');
  host.innerHTML = '';
  if (!devices || devices.length === 0) {
    const empty = document.createElement('div');
    empty.className = 'muted-empty';
    empty.textContent = t('noPairedDevices');
    host.appendChild(empty);
    return;
  }
  for (const d of devices) {
    const row = document.createElement('div');
    row.className = `device-row${d.revoked ? ' revoked' : ''}`;
    row.dataset.deviceId = d.id;

    const info = document.createElement('div');
    info.className = 'device-info';
    const nameEl = document.createElement('strong');
    nameEl.textContent = d.label || d.id;
    const metaEl = document.createElement('span');
    metaEl.className = 'mono device-meta';
    const ipText = d.remote_ip ? `${d.remote_ip} · ` : '';
    const when = d.paired_at ? new Date(d.paired_at * 1000).toLocaleString() : '';
    metaEl.textContent = `${ipText}${when}`;
    info.append(nameEl, metaEl);

    if (d.revoked) {
      const badge = document.createElement('span');
      badge.className = 'mini-btn';
      badge.textContent = t('revokedBadge');
      row.append(info, badge);
    } else {
      const revokeBtn = document.createElement('button');
      revokeBtn.className = 'mini-btn';
      revokeBtn.type = 'button';
      revokeBtn.textContent = t('revoke');
      revokeBtn.addEventListener('click', async () => {
        try {
          await api(`/api/remote/devices/${d.id}`, { method: 'DELETE' });
          showToast(t('revokedDevice'), 'ok');
          await refreshConnectModal(false);
        } catch (err) {
          showToast(err.message, 'err');
        }
      });
      row.append(info, revokeBtn);
    }
    host.appendChild(row);
  }
}

async function issuePairingQr() {
  const custom = $('connect-custom-url').value.trim();
  const selected = $('connect-endpoint-select').value;
  const endpointUrl = custom || selected || null;
  const data = await api('/api/remote/tickets', {
    method: 'POST',
    body: JSON.stringify({ endpoint_url: endpointUrl, ttl_secs: 300 }),
  });
  renderSvgInto($('connect-qr-svg'), data.qr_svg);
  $('connect-pair-url').value = data.pair_url || '';
  startQrCountdown(data.expires_in_secs || 300);
}

async function refreshConnectModal(issueNewQr = true) {
  const status = await api('/api/remote/status');
  $('connect-loopback-warn').classList.toggle('hidden', !status.is_loopback_only);

  const select = $('connect-endpoint-select');
  const prevValue = select.value;
  select.innerHTML = '';
  for (const ep of status.endpoints || []) {
    const opt = document.createElement('option');
    opt.value = ep.url;
    opt.textContent = ep.label;
    select.appendChild(opt);
  }
  if (prevValue && [...select.options].some((o) => o.value === prevValue)) {
    select.value = prevValue;
  }
  renderPairedDevices(status.devices || []);
  if (issueNewQr) {
    await issuePairingQr();
  }
}

$('btn-open-connect').addEventListener('click', async () => {
  $('connect-modal').classList.remove('hidden');
  try {
    await refreshConnectModal(true);
  } catch (err) {
    showToast(err.message, 'err');
  }
});

$('btn-close-connect').addEventListener('click', () => {
  $('connect-modal').classList.add('hidden');
  if (state.qrTimer) {
    clearInterval(state.qrTimer);
    state.qrTimer = null;
  }
});

$('connect-endpoint-select').addEventListener('change', () => issuePairingQr().catch((err) => showToast(err.message, 'err')));
$('btn-refresh-qr').addEventListener('click', () => refreshConnectModal(true).catch((err) => showToast(err.message, 'err')));
$('btn-copy-pair-url').addEventListener('click', async () => {
  const url = $('connect-pair-url').value;
  if (!url) return;
  try {
    await navigator.clipboard.writeText(url);
    showToast(t('linkCopied'), 'ok');
  } catch (_) {
    $('connect-pair-url').select();
    document.execCommand?.('copy');
    showToast(t('linkCopied'), 'ok');
  }
});

// Presets & Settings
function updatePresetSubmitButton() {
  const btn = $('btn-save-preset');
  const cancelBtn = $('btn-cancel-preset-edit');
  const editing = Boolean($('preset-id')?.value);
  if (btn) {
    btn.textContent = editing ? t('saveChanges') : t('savePreset');
  }
  if (cancelBtn) {
    cancelBtn.classList.toggle('hidden', !editing);
  }
}

function resetPresetForm() {
  $('preset-form').reset();
  $('preset-id').value = '';
  $('preset-api-key').placeholder = 'API Key (optional)';
  updatePresetSubmitButton();
}

function populatePresetEdit(p) {
  $('preset-id').value = p.id;
  $('preset-name').value = p.name;
  $('preset-kind').value = p.kind;
  $('preset-model').value = p.model;
  $('preset-base-url').value = p.base_url || '';
  $('preset-api-key').value = '';
  $('preset-api-key').placeholder = p.api_key ? 'Leave blank to keep existing key' : 'API Key (optional)';
  updatePresetSubmitButton();
}

async function loadActiveCliConfigs() {
  const active = await api('/api/presets/active');
  const host = $('active-cli-configs');
  host.innerHTML = '';
  const items = [
    ['Codex', active.codex],
    ['Claude', active.claude],
  ];
  for (const [label, cfg] of items) {
    const card = document.createElement('div');
    card.className = 'active-cli-card';
    const title = document.createElement('strong');
    title.textContent = label;
    const detail = document.createElement('div');
    detail.className = 'mono';
    detail.style.color = '#8b949e';
    detail.style.fontSize = '11px';
    detail.textContent = cfg?.configured
      ? `${cfg.model || '-'} ${cfg.base_url ? `· ${cfg.base_url}` : ''}`
      : t('notConfigured');
    card.append(title, detail);
    host.appendChild(card);
  }
}

async function loadPresets() {
  const data = await api('/api/presets');
  state.presetsCache = data.presets || [];
  const host = $('preset-list');
  host.innerHTML = '';
  for (const p of state.presetsCache) {
    const row = document.createElement('div');
    row.className = 'preset-row';
    const info = document.createElement('span');
    info.innerHTML = `<strong></strong> <span class="mono" style="color:#8b949e"></span>`;
    info.querySelector('strong').textContent = p.name;
    info.querySelector('span').textContent = `(${p.kind} · ${p.model})`;
    const actions = document.createElement('div');
    actions.style.display = 'flex';
    actions.style.gap = '6px';

    const applyBtn = document.createElement('button');
    applyBtn.className = 'mini-btn';
    applyBtn.type = 'button';
    applyBtn.textContent = t('apply');
    applyBtn.onclick = async () => {
      try {
        await api(`/api/presets/${p.id}/apply`, { method: 'POST' });
        showToast(`${t('presetApplied')}: ${p.name}`, 'ok');
        await loadActiveCliConfigs();
      } catch (err) {
        showToast(err.message, 'err');
      }
    };

    const editBtn = document.createElement('button');
    editBtn.className = 'mini-btn';
    editBtn.type = 'button';
    editBtn.textContent = t('edit');
    editBtn.onclick = () => populatePresetEdit(p);

    const delBtn = document.createElement('button');
    delBtn.className = 'mini-btn';
    delBtn.type = 'button';
    delBtn.textContent = '✕';
    delBtn.onclick = async () => {
      await api(`/api/presets/${p.id}`, { method: 'DELETE' });
      if ($('preset-id').value === p.id) resetPresetForm();
      await loadPresets();
    };
    actions.append(applyBtn, editBtn, delBtn);
    row.append(info, actions);
    host.appendChild(row);
  }
}

async function loadSettingsData() {
  await Promise.all([loadPresets(), loadActiveCliConfigs()]);
}

$('btn-open-settings').addEventListener('click', async () => {
  $('settings-modal').classList.remove('hidden');
  resetPresetForm();
  await loadSettingsData();
});
$('btn-close-settings').addEventListener('click', () => $('settings-modal').classList.add('hidden'));
$('btn-cancel-preset-edit').addEventListener('click', resetPresetForm);

async function importPresetFromCli(kind) {
  try {
    const res = await api('/api/presets/import', {
      method: 'POST',
      body: JSON.stringify({ kind }),
    });
    showToast(`${t('importedPrefix')}: ${res.preset?.name || kind}`, 'ok');
    await loadSettingsData();
  } catch (err) {
    showToast(err.message, 'err');
  }
}

$('btn-import-codex').addEventListener('click', () => importPresetFromCli('codex_api'));
$('btn-import-claude').addEventListener('click', () => importPresetFromCli('claude_api'));

$('preset-form').addEventListener('submit', async (e) => {
  e.preventDefault();
  try {
    await api('/api/presets', {
      method: 'POST',
      body: JSON.stringify({
        id: $('preset-id').value || '',
        name: $('preset-name').value,
        kind: $('preset-kind').value,
        model: $('preset-model').value,
        base_url: $('preset-base-url').value,
        api_key: $('preset-api-key').value,
        wire_api: 'responses',
      }),
    });
    resetPresetForm();
    await loadPresets();
  } catch (err) {
    showToast(err.message, 'err');
  }
});

$('password-form').addEventListener('submit', async (e) => {
  e.preventDefault();
  try {
    await api('/api/auth/password', {
      method: 'POST',
      body: JSON.stringify({
        current_password: $('pw-current').value,
        new_password: $('pw-new').value,
      }),
    });
    e.target.reset();
    showToast(t('passwordUpdated'), 'ok');
  } catch (err) {
    showToast(err.message, 'err');
  }
});

bootstrap();
