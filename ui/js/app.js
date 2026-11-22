// The main window: a thin view over the Rust side. It renders the state it
// gets and sends intents; the Rust side validates and saves. All text from
// the state goes into the DOM as text, never as markup.

import { connect, request, notify, onEvent } from './bridge.js';

const $ = (id) => document.getElementById(id);
const params = new URLSearchParams(location.search);

let state = null;
let selected = null;
let unassigned = [];

const actionLabel = { start: 'Start', stop: 'Stop', suspend: 'Suspend', ignore: 'Ignore' };

function el(tag, props = {}, ...children) {
  const node = document.createElement(tag);
  for (const [key, value] of Object.entries(props)) {
    if (key === 'class') node.className = value;
    else if (key === 'text') node.textContent = value;
    else if (key.startsWith('on')) node.addEventListener(key.slice(2), value);
    else if (value === true) node.setAttribute(key, '');
    else if (value !== false && value != null) node.setAttribute(key, value);
  }
  for (const child of children) if (child != null) node.append(child);
  return node;
}

function setStatus(text, isError = false) {
  const status = $('status');
  status.textContent = text || '';
  status.className = isError ? 'error-text' : '';
}

/** Sends a request, takes over the returned state, reports errors. */
async function call(type, fields = {}) {
  const reply = await request(type, fields);
  if (reply.state) adopt(reply.state);
  if (!reply.ok) setStatus(reply.error, true);
  return reply;
}

function adopt(next) {
  state = next;
  const names = state.profiles.map((p) => p.name);
  if (!selected || !names.some((n) => n.toLowerCase() === selected.toLowerCase())) {
    selected = (state.profiles.find((p) => p.active) || state.profiles[0] || {}).name || null;
  }
  render();
}

function current() {
  return state.profiles.find((p) => p.name.toLowerCase() === (selected || '').toLowerCase()) || null;
}

function render() {
  $('active-badge').replaceChildren('Active: ', el('strong', { text: state.active }));
  $('autostart-wrap').hidden = state.autostart === null;
  $('autostart').checked = !!state.autostart;
  $('config-file').textContent = state.config_file;
  setStatus(state.status);
  renderUpdate();
  renderProfiles();
  renderDetail();
}

function when(seconds) {
  return new Date(seconds * 1000).toLocaleString([], { dateStyle: 'medium', timeStyle: 'short' });
}

/** UP1: version, what the last check found, the mode and the two buttons. */
function renderUpdate() {
  const u = state.update;
  $('update-version').textContent = `Version ${u.version}`;
  let text;
  if (u.checking) text = 'Checking for a newer version…';
  else if (u.error) text = `Last check failed: ${u.error}`;
  else if (u.available && u.blocked) text = `Version ${u.available} is available; ${u.blocked}.`;
  else if (u.available && u.failed) text = `Version ${u.available} did not start here last time; it is installed only on request.`;
  else if (u.available) text = `Version ${u.available} is available.`;
  else if (u.last_check) text = `Up to date (checked ${when(u.last_check)}).`;
  else text = 'Not checked yet.';
  $('update-state').textContent = text;
  const mode = $('update-mode');
  if (document.activeElement !== mode) mode.value = u.mode;
  mode.disabled = u.mode_locked;
  $('update-locked').hidden = !u.mode_locked;
  $('check-updates').disabled = u.checking;
  const install = $('install-update');
  install.hidden = !u.available || !!u.blocked;
  install.textContent = u.available ? `Install ${u.available}` : 'Install';
}

function renderProfiles() {
  const list = $('profile-list');
  list.replaceChildren(...state.profiles.map((profile) => {
    const isSelected = profile === current();
    return el('li', {}, el('button', {
      type: 'button',
      'aria-current': isSelected ? 'true' : 'false',
      onclick: () => select(profile.name),
    }, el('span', { class: 'name', text: profile.name }),
    profile.active ? el('span', { class: 'tag', text: 'active' }) : null));
  }));
}

async function select(name) {
  selected = name;
  render();
  await loadUnassigned();
}

function optionsText(e) {
  const parts = [];
  if (e.action === 'suspend') parts.push(e.trim ? 'trim' : 'no trim');
  if (e.action === 'stop') {
    if (e.on_timeout === 'keep') parts.push('keep if it hangs');
    if (e.restart_on_neutral) parts.push('restarts on neutral');
  }
  if (e.args.length) parts.push(`args: ${e.args.join(' ')}`);
  if (e.cwd) parts.push(`in ${e.cwd}`);
  if (e.confirmed) parts.push('confirmed');
  if (e.protection === 'warn' && !e.confirmed && e.action !== 'ignore' && e.action !== 'start') parts.push('needs confirmation');
  return parts.join(' · ');
}

function renderDetail() {
  const profile = current();
  $('empty').hidden = !!profile;
  $('profile').hidden = !profile;
  if (!profile) return;
  $('profile-name').textContent = profile.name;
  $('profile-meta').textContent = profile.active
    ? 'This profile is active.'
    : `${profile.entries.length} program${profile.entries.length === 1 ? '' : 's'}`;
  const hotkey = $('hotkey');
  if (document.activeElement !== hotkey) hotkey.value = profile.hotkey || '';
  hotkey.removeAttribute('aria-invalid');
  $('apply').disabled = state.applying;
  $('apply').textContent = state.applying ? 'Applying…' : 'Apply profile';

  const rows = profile.entries.map((e, index) => el('tr', {},
    el('td', {}, el('span', { class: 'exe', text: e.exe }), e.path ? el('span', { class: 'path', text: e.path }) : null),
    el('td', {}, el('span', { class: `action ${e.action}`, text: actionLabel[e.action] })),
    el('td', { class: 'opts', text: optionsText(e) }),
    el('td', {},
      el('button', { type: 'button', class: 'button small', 'aria-label': `Edit ${e.exe}`, onclick: () => editEntry(index), text: 'Edit' }),
      el('button', { type: 'button', class: 'button small danger', 'aria-label': `Remove ${e.exe}`, onclick: () => removeEntry(index), text: 'Remove' }))));
  $('entries').tBodies[0].replaceChildren(...rows);
  $('entries').hidden = rows.length === 0;
  $('entries-empty').hidden = rows.length !== 0;
  renderUnassigned();
}

function renderUnassigned() {
  const rows = unassigned.map((u) => el('tr', {},
    el('td', {}, el('span', { class: 'exe', text: u.exe }), u.path ? el('span', { class: 'path', text: u.path }) : null),
    el('td', {}, u.new ? el('span', { class: 'tag new', text: 'new' }) : 'earlier'),
    el('td', {},
      el('button', { type: 'button', class: 'button small', 'aria-label': `Add ${u.exe}`, onclick: () => addProgram(u.exe, null), text: 'Add' }),
      el('button', { type: 'button', class: 'button small', 'aria-label': `Dismiss ${u.exe}`, onclick: () => dismiss(u.exe), text: 'Dismiss' }))));
  $('unassigned').tBodies[0].replaceChildren(...rows);
  $('unassigned').hidden = rows.length === 0;
  $('unassigned-empty').hidden = rows.length !== 0;
}

async function loadUnassigned() {
  const profile = current();
  unassigned = [];
  if (profile) {
    const reply = await call('unassigned', { profile: profile.name });
    unassigned = reply.ok ? reply.data : [];
  }
  renderUnassigned();
}

// Dialogs -----------------------------------------------------------------

function openDialog(dialog) {
  dialog.showModal();
}

for (const button of document.querySelectorAll('[data-close]')) {
  button.addEventListener('click', () => button.closest('dialog').close());
}

/** Resolves with true when the user confirms. */
function confirmDialog(title, text, yes = 'Yes') {
  return new Promise((resolve) => {
    const dialog = $('confirm-dialog');
    $('confirm-title').textContent = title;
    $('confirm-text').textContent = text;
    $('confirm-ok').textContent = yes;
    const done = (answer) => {
      dialog.removeEventListener('close', onClose);
      $('confirm-ok').onclick = null;
      resolve(answer);
    };
    const onClose = () => done(false);
    dialog.addEventListener('close', onClose);
    $('confirm-ok').onclick = () => { dialog.removeEventListener('close', onClose); dialog.close(); done(true); };
    openDialog(dialog);
    $('confirm-ok').focus();
  });
}

/** Asks for a profile name; `action(name)` returns the reply. */
function nameDialog(title, initial, action) {
  const dialog = $('name-dialog');
  $('name-title').textContent = title;
  $('name-input').value = initial;
  $('name-error').textContent = '';
  $('name-form').onsubmit = async (event) => {
    event.preventDefault();
    const reply = await action($('name-input').value);
    if (reply.ok) dialog.close();
    else $('name-error').textContent = reply.error;
  };
  openDialog(dialog);
  $('name-input').select();
}

// Entry editing -------------------------------------------------------------

let editing = null;

function showOptions() {
  const action = $('e-action').value;
  const restart = $('e-restart').checked;
  const entry = editing ? current().entries[editing.index] : null;
  const form = $('entry-form');
  form.querySelector('[data-for="stop"]').hidden = action !== 'stop';
  form.querySelector('[data-for="launch"]').hidden = !(action === 'start' || (action === 'stop' && restart));
  form.querySelector('[data-for="suspend"]').hidden = action !== 'suspend';
  const warn = entry && entry.protection === 'warn' && (action === 'stop' || action === 'suspend');
  form.querySelector('[data-for="confirm"]').hidden = !warn;
  $('e-warn').textContent = warn ? `${$('e-exe').value} is on the warn list (remote access, drivers, security or developer tools).` : '';
}

function editEntry(index) {
  const e = current().entries[index];
  editing = { index };
  $('entry-title').textContent = `Edit ${e.exe}`;
  $('e-exe').value = e.exe;
  $('e-path').value = e.path || '';
  $('e-action').value = e.action;
  $('e-timeout').value = e.on_timeout;
  $('e-restart').checked = e.restart_on_neutral;
  $('e-args').value = e.args.join('\n');
  $('e-cwd').value = e.cwd || '';
  $('e-trim').checked = e.trim ?? state.default_trim;
  $('e-confirmed').checked = e.confirmed;
  $('entry-error').textContent = '';
  showOptions();
  openDialog($('entry-dialog'));
}

$('e-action').addEventListener('change', showOptions);
$('e-restart').addEventListener('change', showOptions);

$('entry-form').addEventListener('submit', async (event) => {
  event.preventDefault();
  const action = $('e-action').value;
  const entry = {
    exe: $('e-exe').value.trim(),
    path: $('e-path').value.trim() || null,
    action,
    args: $('e-args').value.split('\n').map((a) => a.trim()).filter(Boolean),
    cwd: $('e-cwd').value.trim() || null,
    on_timeout: $('e-timeout').value,
    restart_on_neutral: action === 'stop' && $('e-restart').checked,
    trim: action === 'suspend' ? $('e-trim').checked : null,
    confirmed: $('e-confirmed').checked,
  };
  const reply = await call('update_entry', { profile: current().name, index: editing.index, entry });
  if (reply.ok) $('entry-dialog').close();
  else $('entry-error').textContent = reply.error;
});

async function removeEntry(index) {
  const e = current().entries[index];
  if (await confirmDialog('Remove program', `Remove ${e.exe} from "${current().name}"? The program itself is not changed.`, 'Remove')) {
    await call('remove_entry', { profile: current().name, index });
    await loadUnassigned();
  }
}

// Adding programs ---------------------------------------------------------

/** M4: protected programs are refused, warn-list programs need a yes. */
async function addProgram(exe, path) {
  const check = await call('check_program', { exe });
  if (!check.ok) return false;
  let confirmed = false;
  if (check.data.protection === 'hard') {
    $('add-error').textContent = check.data.message;
    setStatus(check.data.message, true);
    return false;
  }
  if (check.data.protection === 'warn') {
    confirmed = await confirmDialog('Warn list', check.data.message, 'Add anyway');
    if (!confirmed) return false;
  }
  const reply = await call('add_entry', { profile: current().name, exe, path, confirmed });
  if (reply.ok) await loadUnassigned();
  else $('add-error').textContent = reply.error;
  return reply.ok;
}

function megabytes(bytes) {
  return `${(bytes / 1048576).toFixed(1)} MB`;
}

const protectionText = { none: '', warn: 'warn list', hard: 'protected, cannot be added' };

async function openAdd() {
  $('add-error').textContent = '';
  const reply = await call('running');
  const running = reply.ok ? reply.data : [];
  const rows = running.map((p) => el('tr', {},
    el('td', { class: 'exe', text: p.exe }),
    el('td', { class: 'num', text: megabytes(p.private_bytes) }),
    el('td', { class: `protection ${p.protection}`, text: protectionText[p.protection] }),
    el('td', {}, el('button', {
      type: 'button', class: 'button small', disabled: p.protection === 'hard',
      'aria-label': `Add ${p.exe}`,
      onclick: async () => { if (await addProgram(p.exe, null)) $('add-dialog').close(); },
      text: 'Add',
    }))));
  $('running').tBodies[0].replaceChildren(...rows);
  openDialog($('add-dialog'));
}

$('pick-file').addEventListener('click', async () => {
  const reply = await call('pick_executable');
  if (reply.ok && reply.data && await addProgram(reply.data.exe, reply.data.path)) $('add-dialog').close();
});

async function dismiss(exe) {
  await call('dismiss', { profile: current().name, exe });
  await loadUnassigned();
}

// Profile commands ----------------------------------------------------------

$('new-profile').addEventListener('click', () => nameDialog('New profile', '', async (name) => {
  const reply = await call('create_profile', { name });
  if (reply.ok) await select(name.trim());
  return reply;
}));

$('rename').addEventListener('click', () => {
  const from = current().name;
  nameDialog('Rename profile', from, async (to) => {
    const reply = await call('rename_profile', { from, to });
    if (reply.ok) await select(to.trim());
    return reply;
  });
});

$('copy').addEventListener('click', () => {
  const from = current().name;
  nameDialog('Copy profile', `${from} copy`, async (to) => {
    const reply = await call('copy_profile', { from, to });
    if (reply.ok) await select(to.trim());
    return reply;
  });
});

$('delete').addEventListener('click', async () => {
  const profile = current();
  const text = profile.active
    ? `"${profile.name}" is active. kaltstellen switches to Neutral first, then deletes it.`
    : `Delete "${profile.name}" and its program list?`;
  if (await confirmDialog('Delete profile', text, 'Delete')) {
    await call('delete_profile', { name: profile.name });
  }
});

$('apply').addEventListener('click', () => call('apply', { profile: current().name }));

$('hotkey-form').addEventListener('submit', async (event) => {
  event.preventDefault();
  const reply = await call('set_hotkey', { profile: current().name, hotkey: $('hotkey').value.trim() || null });
  if (!reply.ok) $('hotkey').setAttribute('aria-invalid', 'true');
});

$('add-program').addEventListener('click', openAdd);
$('refresh-unassigned').addEventListener('click', loadUnassigned);
$('autostart').addEventListener('change', (event) => call('set_autostart', { on: event.target.checked }));
$('update-mode').addEventListener('change', (event) => call('set_update_mode', { mode: event.target.value }));
$('check-updates').addEventListener('click', () => call('check_updates'));
$('install-update').addEventListener('click', () => call('install_update'));

onEvent(async (event) => {
  if (event.event === 'applied') {
    adopt(event.state);
    await loadUnassigned();
  }
  if (event.event === 'update_checked') adopt(event.state);
});

// Test driver: runs bridge requests in order (kaltstellen gui --drive).
globalThis.kaltstellenDrive = async (steps) => {
  const results = [];
  for (const step of steps) {
    const { type, ...fields } = step;
    const reply = await request(type, fields);
    let event = null;
    if (type === 'apply' && reply.ok) {
      event = await new Promise((resolve) => onEvent((e) => { if (e.event === 'applied') resolve(e); }));
    }
    adopt((event || reply).state);
    results.push({ type, ok: reply.ok, error: reply.error || null, status: (event || reply).state.status });
  }
  await loadUnassigned();
  await settle();
  notify('drive_done', { results });
};

function settle() {
  // Two frames, or 200 ms when frames do not come (a covered window).
  return new Promise((resolve) => {
    setTimeout(resolve, 200);
    requestAnimationFrame(() => requestAnimationFrame(resolve));
  });
}

/** Screenshot views: ?view=entry|add|confirm|new opens that dialog. */
async function openView(view) {
  const profile = current();
  if (view === 'entry' && profile && profile.entries.length) editEntry(Math.min(2, profile.entries.length - 1));
  if (view === 'add' && profile) await openAdd();
  if (view === 'confirm' && profile) confirmDialog('Warn list', 'anydesk.exe is a remote-access tool. Freezing or closing it can cut you off or break things. Add it anyway?', 'Add anyway');
  if (view === 'new') nameDialog('New profile', 'Evening', async () => ({ ok: false, error: 'KS205: profile name "Evening" is used already' }));
}

// Errors of the page go to the host's log (and the console in a browser).
function report(message) {
  console.error(message);
  try { notify('log', { message: String(message).slice(0, 2000) }); } catch { /* not connected yet */ }
}
addEventListener('error', (event) => report(event.message));
addEventListener('unhandledrejection', (event) => report(event.reason && event.reason.stack || event.reason));

async function start() {
  const theme = params.get('theme');
  if (theme === 'light' || theme === 'dark') document.documentElement.dataset.theme = theme;
  const mode = await connect();
  document.documentElement.dataset.backend = mode;
  const reply = await request('get_state');
  adopt(reply.state);
  await loadUnassigned();
  await openView(params.get('view'));
  await settle();
  notify('ready');
}

start();
