const invoke = window.__TAURI__.core.invoke;

const translations = {
  en: {
    eyebrow: 'OpenBitFun maintenance', title: 'Import data from BitFun',
    intro: 'Choose what to bring forward. Your original BitFun data will not be deleted.',
    stepSource: 'Step 1', sourceTitle: 'Legacy source', firstLaunch: 'First launch',
    choiceTitle: 'What would you like to do?',
    choiceHelp: 'Migration runs only after OpenBitFun and other data writers have stopped.',
    migrateNow: 'Migrate now', remindLater: 'Remind me later', doNotRemind: 'Do not remind me',
    stepScope: 'Step 2', scopeTitle: 'Choose migration scope', scan: 'Scan selected data',
    stepReview: 'Step 3', reviewTitle: 'Review scan', prepare: 'Run preflight plan',
    stepConfirm: 'Step 4', planTitle: 'Confirm migration', retryWriters: 'Check processes again',
    start: 'Start migration', stepProgress: 'Step 5', progressTitle: 'Migration progress',
    phase: 'Phase', domain: 'Domain', count: 'Completed steps',
    cancel: 'Cancel at a safe boundary', stepDone: 'Result', reportTitle: 'Migration report',
    reportPrivacy: 'This summary contains counts and result codes, not credentials or user content.',
    openDesktop: 'Open OpenBitFun', ready: 'Ready', unsupported: 'Unsupported', missing: 'Not found',
    sourceFound: 'BitFun {version} was found. The source stays read-only.',
    recovery: 'A previous migration journal was found and can be resumed.',
    blockers: '{count} data-writing process(es) must stop before migration can continue.',
    noBlockers: 'No data-writing processes are blocking migration.',
    steps: '{count} ordered domain step(s)', conflicts: '{count} visible conflict(s)',
    imported: 'imported', skipped: 'skipped', warnings: 'warnings',
  },
  'zh-CN': {
    eyebrow: 'OpenBitFun 数据维护', title: '从 BitFun 导入数据',
    intro: '选择要迁移的内容。原始 BitFun 数据不会被删除。', stepSource: '第 1 步',
    sourceTitle: '旧版数据来源', firstLaunch: '首次启动', choiceTitle: '你希望如何处理？',
    choiceHelp: '迁移只会在 OpenBitFun 和其他数据写入进程停止后运行。', migrateNow: '立即迁移',
    remindLater: '稍后提醒', doNotRemind: '不再提醒', stepScope: '第 2 步',
    scopeTitle: '选择迁移范围', scan: '扫描所选数据', stepReview: '第 3 步',
    reviewTitle: '检查扫描结果', prepare: '运行迁移预检', stepConfirm: '第 4 步',
    planTitle: '确认迁移', retryWriters: '重新检查进程', start: '开始迁移',
    stepProgress: '第 5 步', progressTitle: '迁移进度', phase: '阶段', domain: '领域',
    count: '已完成步骤', cancel: '在安全边界取消', stepDone: '结果', reportTitle: '迁移报告',
    reportPrivacy: '此摘要仅包含计数和结果码，不包含凭据或用户正文。', openDesktop: '打开 OpenBitFun',
    ready: '可迁移', unsupported: '不受支持', missing: '未发现',
    sourceFound: '已发现 BitFun {version}。迁移期间来源保持只读。',
    recovery: '发现上次迁移日志，可以从安全状态继续。',
    blockers: '迁移前还需停止 {count} 个数据写入进程。', noBlockers: '没有进程阻止迁移。',
    steps: '{count} 个有序领域步骤', conflicts: '{count} 个可见冲突',
    imported: '已导入', skipped: '已跳过', warnings: '警告',
  },
  'zh-TW': {
    eyebrow: 'OpenBitFun 資料維護', title: '從 BitFun 匯入資料',
    intro: '選擇要遷移的內容。原始 BitFun 資料不會被刪除。', stepSource: '第 1 步',
    sourceTitle: '舊版資料來源', firstLaunch: '首次啟動', choiceTitle: '你希望如何處理？',
    choiceHelp: '遷移只會在 OpenBitFun 和其他資料寫入程序停止後執行。', migrateNow: '立即遷移',
    remindLater: '稍後提醒', doNotRemind: '不再提醒', stepScope: '第 2 步',
    scopeTitle: '選擇遷移範圍', scan: '掃描所選資料', stepReview: '第 3 步',
    reviewTitle: '檢查掃描結果', prepare: '執行遷移預檢', stepConfirm: '第 4 步',
    planTitle: '確認遷移', retryWriters: '重新檢查程序', start: '開始遷移',
    stepProgress: '第 5 步', progressTitle: '遷移進度', phase: '階段', domain: '領域',
    count: '已完成步驟', cancel: '在安全邊界取消', stepDone: '結果', reportTitle: '遷移報告',
    reportPrivacy: '此摘要僅包含計數和結果碼，不包含憑據或使用者正文。', openDesktop: '開啟 OpenBitFun',
    ready: '可遷移', unsupported: '不支援', missing: '未發現',
    sourceFound: '已發現 BitFun {version}。遷移期間來源保持唯讀。',
    recovery: '發現上次遷移日誌，可以從安全狀態繼續。',
    blockers: '遷移前還需停止 {count} 個資料寫入程序。', noBlockers: '沒有程序阻止遷移。',
    steps: '{count} 個有序領域步驟', conflicts: '{count} 個可見衝突',
    imported: '已匯入', skipped: '已略過', warnings: '警告',
  },
};

const locale = navigator.language.startsWith('zh-TW') || navigator.language.startsWith('zh-HK')
  ? 'zh-TW'
  : navigator.language.startsWith('zh') ? 'zh-CN' : 'en';
const text = translations[locale];
document.documentElement.lang = locale;
document.querySelectorAll('[data-i18n]').forEach((node) => {
  node.textContent = text[node.dataset.i18n] || translations.en[node.dataset.i18n];
});

const groups = [
  ['settings_and_credentials', {
    en: ['Settings and credentials', 'Settings are imported; credentials that cannot be decrypted are marked for sign-in.'],
    'zh-CN': ['设置与服务凭据', '导入设置；无法解密的凭据会标记为需要重新登录。'],
    'zh-TW': ['設定與服務憑據', '匯入設定；無法解密的憑據會標記為需要重新登入。'],
  }],
  ['agents_skills_and_miniapps', {
    en: ['Agents, Skills, and MiniApps', 'User content is imported; system Skills and built-in MiniApps are excluded.'],
    'zh-CN': ['Agents、Skills 与 MiniApps', '导入用户内容；排除系统 Skills 和内置 MiniApps。'],
    'zh-TW': ['Agents、Skills 與 MiniApps', '匯入使用者內容；排除系統 Skills 和內建 MiniApps。'],
  }],
  ['workspaces_sessions_and_tasks', {
    en: ['Workspaces, sessions, and tasks', 'Includes coordination.sqlite as a required atomic dependency.'],
    'zh-CN': ['工作区、会话与 Agent 任务状态', '强制包含 coordination.sqlite 作为原子依赖。'],
    'zh-TW': ['工作區、工作階段與 Agent 任務狀態', '強制包含 coordination.sqlite 作為原子相依項。'],
  }],
  ['memory', {
    en: ['Memory', 'Imports structured and file-backed memory without user-content telemetry.'],
    'zh-CN': ['记忆', '导入结构化与文件记忆，不发送用户正文遥测。'],
    'zh-TW': ['記憶', '匯入結構化與檔案記憶，不傳送使用者正文遙測。'],
  }],
  ['remote_connections_and_devices', {
    en: ['Remote connections and devices', 'Non-portable identities and secrets are marked for repair or sign-in.'],
    'zh-CN': ['远程连接与设备', '不可移植的身份和秘密会标记为需要修复或重新登录。'],
    'zh-TW': ['遠端連線與裝置', '不可移植的身分和秘密會標記為需要修復或重新登入。'],
  }],
];

let current;
let pollTimer;

function format(template, values) {
  return Object.entries(values).reduce((value, [key, replacement]) =>
    value.replace(`{${key}}`, String(replacement)), template);
}

function show(id, visible = true) {
  document.getElementById(id).hidden = !visible;
}

function setBusy(busy) {
  document.querySelectorAll('button').forEach((button) => { button.disabled = busy; });
}

function notice(message) {
  const node = document.getElementById('notice');
  node.textContent = message || '';
  node.hidden = !message;
}

function row(title, detail) {
  const item = document.createElement('div');
  item.className = 'result-row';
  const strong = document.createElement('strong');
  strong.textContent = title;
  const small = document.createElement('small');
  small.textContent = detail;
  item.append(strong, small);
  return item;
}

function renderScopes(selection) {
  const list = document.getElementById('scope-list');
  list.replaceChildren();
  const selected = new Set(selection?.groups?.length ? selection.groups : groups.map(([id]) => id));
  groups.forEach(([id, labels]) => {
    const option = document.createElement('div');
    option.className = 'scope-option';
    const checkbox = document.createElement('input');
    checkbox.type = 'checkbox';
    checkbox.id = `scope-${id}`;
    checkbox.value = id;
    checkbox.checked = selected.has(id);
    if (current?.mode === 'execute') checkbox.disabled = true;
    const label = document.createElement('label');
    label.htmlFor = checkbox.id;
    const strong = document.createElement('strong');
    const description = document.createElement('span');
    [strong.textContent, description.textContent] = labels[locale] || labels.en;
    label.append(strong, description);
    option.append(checkbox, label);
    list.append(option);
  });
}

function selection() {
  return {
    groups: [...document.querySelectorAll('#scope-list input:checked')].map((input) => input.value),
  };
}

function render(view) {
  current = view;
  const source = view.source;
  document.getElementById('source-badge').textContent = !source
    ? text.missing : source.supported ? text.ready : text.unsupported;
  document.getElementById('source-summary').textContent = source
    ? format(text.sourceFound, { version: source.productVersion }) : text.missing;
  document.getElementById('source-path').textContent = source?.roots?.[0]?.displayPath || '';
  notice(view.error?.message || (view.recovery ? text.recovery : ''));

  show('choice-card', view.mode === 'onboarding' && !view.findings.length && !view.plan && !view.running);
  show('scope-card', Boolean(source) && (view.mode === 'execute' || view.findings.length || view.plan));
  renderScopes(view.selection);

  const findings = document.getElementById('findings');
  findings.replaceChildren(...view.findings.map((finding) =>
    row(finding.code, `${finding.entityCount} item(s), ${finding.logicalBytes} byte(s)`)));
  show('scan-card', view.findings.length > 0 && !view.plan);

  const planSummary = document.getElementById('plan-summary');
  if (view.plan) {
    planSummary.replaceChildren(
      row(text.steps.replace('{count}', view.plan.steps.length), view.plan.planHash),
      row(text.conflicts.replace('{count}', view.plan.conflicts.length), `${view.plan.estimatedWriteBytes} byte(s)`),
    );
  }
  show('plan-card', Boolean(view.plan) && !view.running && !view.report);
  const blocker = document.getElementById('blockers');
  blocker.textContent = view.blockers.length
    ? format(text.blockers, { count: view.blockers.length }) : text.noBlockers;
  blocker.hidden = !view.blockers.length;
  show('retry-writers', view.blockers.length > 0);

  const progress = view.progress;
  show('progress-card', Boolean(progress) && (view.running || view.status === 'cancelled'));
  if (progress) {
    document.getElementById('phase').textContent = progress.phase;
    document.getElementById('domain').textContent = progress.domain || '-';
    document.getElementById('count').textContent = `${progress.processed} / ${progress.total}`;
    document.getElementById('progress-message').textContent = progress.code.replaceAll('_', ' ');
    document.getElementById('cancel').disabled = !view.running;
  }

  const reportSummary = document.getElementById('report-summary');
  if (view.report) {
    reportSummary.replaceChildren(...view.report.domainResults.map((result) =>
      row(result.domain, `${result.imported} ${text.imported}, ${result.skipped} ${text.skipped}, ${result.warnings.length} ${text.warnings}`)));
  }
  show('report-card', !view.running && (Boolean(view.report) || view.status === 'cancelled'));
  document.getElementById('start').disabled = !view.canExecute;

  if (view.running && !pollTimer) {
    pollTimer = window.setInterval(refresh, 500);
  } else if (!view.running && pollTimer) {
    window.clearInterval(pollTimer);
    pollTimer = undefined;
  }
}

async function call(command, request = {}) {
  setBusy(true);
  try {
    const result = await invoke(command, { request });
    if (result) render(result);
    return result;
  } catch (error) {
    notice(error?.message || String(error));
    return undefined;
  } finally {
    setBusy(false);
    if (current) render(current);
  }
}

async function refresh() {
  try {
    render(await invoke('get_migrator_bootstrap', { request: {} }));
  } catch {
    if (pollTimer) window.clearInterval(pollTimer);
    pollTimer = undefined;
  }
}

document.getElementById('migrate-now').addEventListener('click', () => {
  show('choice-card', false);
  show('scope-card');
  renderScopes(current.selection);
});
document.getElementById('remind-later').addEventListener('click', () =>
  call('finish_legacy_migration', { choice: 'remind_later' }));
document.getElementById('do-not-remind').addEventListener('click', () =>
  call('finish_legacy_migration', { choice: 'do_not_remind' }));
document.getElementById('scan').addEventListener('click', () =>
  call('scan_legacy_migration', { selection: selection() }));
document.getElementById('prepare').addEventListener('click', () =>
  call('prepare_legacy_migration', { selection: selection() }));
document.getElementById('retry-writers').addEventListener('click', () =>
  call('retry_writer_check'));
document.getElementById('start').addEventListener('click', () =>
  call('start_legacy_migration', { planHash: current.plan.planHash }));
document.getElementById('cancel').addEventListener('click', () =>
  call('cancel_legacy_migration'));
document.getElementById('open-desktop').addEventListener('click', () =>
  call('finish_legacy_migration', { choice: current.report ? 'migrate_now' : 'remind_later' }));

refresh().then(() => {
  if (current?.mode === 'execute') show('scope-card');
});
