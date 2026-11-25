export const I18N = {
  en: {
    subtitle: 'Keep native Claude Code, Codex & shell sessions alive in tmux.',
    username: 'Username',
    password: 'Password',
    passwordPlaceholder: 'Check .hqcode-initial-password on first run',
    signIn: 'Sign in to HQ',
    gitDiff: 'Git Diff',
    presetsSecurity: 'Presets & Security',
    logout: 'Logout',
    quickLaunch: 'Quick Launch',
    liveSessions: 'Live Sessions',
    newShell: '+ Shell',
    files: 'Files',
    searchPlaceholder: 'Search session output (Ctrl+K)...',
    emptyTitle: "Your Coding Agents' Headquarters",
    emptyDesc: 'Launch a persistent tmux session from the sidebar. Close your browser or switch to your phone anytime without losing context.',
    save: 'Save',
    close: 'Close',
    gitReview: 'Git Review',
    allChanges: 'All Changes',
    commitPlaceholder: 'Commit message...',
    commit: 'Commit',
    workingTreeClean: 'Working tree clean.',
    noDiff: 'No diff content.',
    revert: 'Revert',
    confirmRevert: 'Confirm?',
    jumpBottom: '↓ Jump to bottom',
    live: 'Live',
    reconnecting: 'Reconnecting...',
    sendSteer: 'Send steer / prompt...',
    send: 'Send',
    presetsTitle: 'Model Presets (real config sync)',
    changePasswordTitle: 'Change Password',
    savePreset: 'Save Preset',
    updatePassword: 'Update Password',
    apply: 'Apply',
    presetApplied: 'Preset applied to shared CLI config',
    passwordUpdated: 'Password updated successfully',
    fileSaved: 'File saved atomically',
    committedPrefix: 'Committed',
    revertedPrefix: 'Reverted',
    terminalHere: 'Open terminal here',
  },
  zh: {
    subtitle: '在原生 tmux 中常驻运行 Claude Code、Codex 与 Shell 会话。',
    username: '用户名',
    password: '密码',
    passwordPlaceholder: '首次运行请查看 .hqcode-initial-password',
    signIn: '登录指挥部',
    gitDiff: 'Git 变更',
    presetsSecurity: '预设与安全',
    logout: '退出',
    quickLaunch: '快速启动',
    liveSessions: '活跃会话',
    newShell: '+ 终端',
    files: '工作区文件',
    searchPlaceholder: '搜索会话输出 (Ctrl+K)...',
    emptyTitle: '你的 Coding Agents 总指挥部',
    emptyDesc: '从左侧启动持久 tmux 会话。关闭浏览器或切到手机随时无缝接管，现场永不丢失。',
    save: '保存',
    close: '关闭',
    gitReview: 'Git 变更审阅',
    allChanges: '全部改动',
    commitPlaceholder: '输入提交说明...',
    commit: '提交',
    workingTreeClean: '工作区干净，暂无未提交改动。',
    noDiff: '暂无 Diff 内容。',
    revert: '还原',
    confirmRevert: '确认还原?',
    jumpBottom: '↓ 回到底部',
    live: '已连接',
    reconnecting: '重连中...',
    sendSteer: '发送纠正或指令...',
    send: '发送',
    presetsTitle: '模型预设（直写共享 CLI 配置）',
    changePasswordTitle: '修改登录密码',
    savePreset: '保存预设',
    updatePassword: '更新密码',
    apply: '应用',
    presetApplied: '已应用预设到真实 CLI 配置',
    passwordUpdated: '密码已更新',
    fileSaved: '文件已原子保存',
    committedPrefix: '已提交',
    revertedPrefix: '已还原',
    terminalHere: '在此目录打开终端',
  },
};

export function detectDefaultLang(navLang) {
  if (typeof navLang === 'string' && navLang.toLowerCase().startsWith('zh')) {
    return 'zh';
  }
  return 'en';
}

export function escapeHtml(str) {
  return String(str)
    .replaceAll('&', '&amp;')
    .replaceAll('<', '&lt;')
    .replaceAll('>', '&gt;')
    .replaceAll('"', '&quot;')
    .replaceAll("'", '&#39;');
}

export function parseUnifiedDiffLines(diffText) {
  if (!diffText) return [];
  const rows = [];
  let oldLine = 0;
  let newLine = 0;
  for (const raw of diffText.split('\n')) {
    if (raw.startsWith('@@')) {
      const m = raw.match(/@@ -(\d+)(?:,\d+)? \+(\d+)(?:,\d+)? @@/);
      if (m) {
        oldLine = Number.parseInt(m[1], 10);
        newLine = Number.parseInt(m[2], 10);
      }
      rows.push({ kind: 'hunk', oldNum: '', newNum: '', text: raw });
    } else if (
      raw.startsWith('diff ') ||
      raw.startsWith('index ') ||
      raw.startsWith('new file') ||
      raw.startsWith('deleted file') ||
      raw.startsWith('---') ||
      raw.startsWith('+++')
    ) {
      rows.push({ kind: 'meta', oldNum: '', newNum: '', text: raw });
    } else if (raw.startsWith('+')) {
      rows.push({ kind: 'add', oldNum: '', newNum: String(newLine++), text: raw });
    } else if (raw.startsWith('-')) {
      rows.push({ kind: 'del', oldNum: String(oldLine++), newNum: '', text: raw });
    } else {
      const hasNums = oldLine > 0 || newLine > 0;
      rows.push({
        kind: 'ctx',
        oldNum: hasNums ? String(oldLine++) : '',
        newNum: hasNums ? String(newLine++) : '',
        text: raw,
      });
    }
  }
  return rows;
}

export const MOBILE_KEY_MAP = {
  esc: '\x1b',
  tab: '\t',
  ctrlc: '\x03',
  up: '\x1b[A',
  down: '\x1b[B',
  approve: 'y\r',
  reject: 'n\r',
};
