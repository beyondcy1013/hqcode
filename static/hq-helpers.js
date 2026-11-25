export const I18N = {
  en: {
    subtitle: 'Keep native Claude Code, Codex & shell sessions alive in tmux.',
    username: 'Username',
    password: 'Password',
    passwordPlaceholder: 'Check .hqcode-initial-password on first run',
    signIn: 'Sign in to HQ',
    gitDiff: 'Git Diff',
    connect: 'Connect',
    connectTitle: 'Phone & Remote QR Connect',
    connectDesc: 'Scan with your phone camera to pair once and open this workspace directly without typing passwords.',
    loopbackWarn: 'Server is bound to localhost only (127.0.0.1). Restart with --lan or --public-url <URL> so your phone can reach it.',
    endpointLabel: 'Target Endpoint',
    customUrlPlaceholder: 'Optional custom URL (e.g. https://hq.example.com)',
    refreshQr: 'New QR',
    copyLink: 'Copy Link',
    linkCopied: 'Pairing link copied',
    expiresInPrefix: 'Expires in',
    qrExpired: 'QR expired — click New QR',
    pairedDevices: 'Paired Devices',
    noPairedDevices: 'No paired devices yet.',
    revoke: 'Revoke',
    revokedDevice: 'Device access revoked',
    revokedBadge: 'Revoked',
    pairingInProgress: 'Pairing device with HQCode...',
    pairedWelcome: 'Device paired successfully',
    pairingFailedPrefix: 'QR pairing failed',
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
    confirmClose: 'Close?',
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
    liveCliConfig: 'Active Shared CLI Configs',
    importCodex: 'Import ~/.codex',
    importClaude: 'Import ~/.claude',
    importedPrefix: 'Imported preset',
    notConfigured: 'Not configured',
    changePasswordTitle: 'Change Password',
    savePreset: 'Save Preset',
    saveChanges: 'Save Changes',
    edit: 'Edit',
    cancelEdit: 'Cancel Edit',
    updatePassword: 'Update Password',
    apply: 'Apply',
    presetApplied: 'Preset applied to shared CLI config',
    passwordUpdated: 'Password updated successfully',
    fileSaved: 'File saved atomically',
    committedPrefix: 'Committed',
    revertedPrefix: 'Reverted',
    terminalHere: 'Open terminal here',
    launchRoot: 'Root',
    noSessionsOption: 'No active sessions',
  },
  zh: {
    subtitle: '在原生 tmux 中常驻运行 Claude Code、Codex 与 Shell 会话。',
    username: '用户名',
    password: '密码',
    passwordPlaceholder: '首次运行请查看 .hqcode-initial-password',
    signIn: '登录指挥部',
    gitDiff: 'Git 变更',
    connect: '扫码连接',
    connectTitle: '手机扫码与远程连接',
    connectDesc: '使用手机相机扫码即可完成一次性免密配对，直接接管当前终端现场。',
    loopbackWarn: '当前服务仅监听本机回环地址 (127.0.0.1)。请改用 --lan 或 --public-url <URL> 启动以便手机访问。',
    endpointLabel: '连接地址',
    customUrlPlaceholder: '可选自定义 URL（如 https://hq.example.com）',
    refreshQr: '刷新二维码',
    copyLink: '复制链接',
    linkCopied: '已复制配对链接',
    expiresInPrefix: '有效期',
    qrExpired: '二维码已过期 — 请点击刷新',
    pairedDevices: '已配对设备',
    noPairedDevices: '暂无已配对设备。',
    revoke: '撤销',
    revokedDevice: '已撤销设备访问权限',
    revokedBadge: '已撤销',
    pairingInProgress: '正在配对设备并接入 HQCode...',
    pairedWelcome: '扫码配对成功，已自动登录',
    pairingFailedPrefix: '扫码配对失败',
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
    confirmClose: '确认关闭?',
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
    liveCliConfig: '本机当前共享 CLI 配置',
    importCodex: '导入 ~/.codex',
    importClaude: '导入 ~/.claude',
    importedPrefix: '已导入预设',
    notConfigured: '未配置',
    changePasswordTitle: '修改登录密码',
    savePreset: '保存预设',
    saveChanges: '保存修改',
    edit: '编辑',
    cancelEdit: '取消编辑',
    updatePassword: '更新密码',
    apply: '应用',
    presetApplied: '已应用预设到真实 CLI 配置',
    passwordUpdated: '密码已更新',
    fileSaved: '文件已原子保存',
    committedPrefix: '已提交',
    revertedPrefix: '已还原',
    terminalHere: '在此目录打开终端',
    launchRoot: '根目录',
    noSessionsOption: '暂无会话',
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
  ctrld: '\x04',
  left: '\x1b[D',
  right: '\x1b[C',
  up: '\x1b[A',
  down: '\x1b[B',
  approve: 'y\r',
  reject: 'n\r',
};
