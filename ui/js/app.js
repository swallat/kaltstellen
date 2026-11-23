// The main window: a thin view over the Rust side. It renders the state it
// gets and sends intents; the Rust side validates and saves. All text from
// the state goes into the DOM as text, never as markup.

import { connect, request, requestWithFiles, notify, onEvent } from './bridge.js';

const $ = (id) => document.getElementById(id);
const params = new URLSearchParams(location.search);

let state = null;
let selected = null;
let unassigned = [];
/** U4: unsaved input of the trigger fields, `{profile, triggers, priority, invalid}`. */
let triggerDraft = null;

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

/** A message of the page itself (an error, a refusal) in the status line.
 * It stays until the next action, so a refresh of the state (focus) does
 * not replace it with the host's status. */
let localStatus = null;

function notice(text, isError = true) {
  localStatus = { text, isError };
  setStatus(text, isError);
}

/** Sends a request, takes over the returned state, reports errors. A
 * `quiet` request (a list being loaded) is not a new action of the user:
 * the page's own message stays. */
async function call(type, fields = {}, { quiet = false } = {}) {
  if (!quiet) localStatus = null;
  const reply = await request(type, fields);
  if (!reply.ok) localStatus = { text: reply.error, isError: true };
  if (reply.state) adopt(reply.state);
  else if (!reply.ok) setStatus(reply.error, true);
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
  if (localStatus) setStatus(localStatus.text, localStatus.isError);
  else setStatus(state.status);
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
  else if (u.available && u.failed && u.retry_at && u.mode === 'auto') text = `Version ${u.available} could not be installed here last time; it is tried again automatically after ${when(u.retry_at)}, or now with Install.`;
  else if (u.available && u.failed) text = `Version ${u.available} could not be installed here last time.`;
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


// Program icons (M2) ----------------------------------------------------------

/** CSS pixels of a program icon; the host gets device pixels. */
const ICON_SIZE = 20;
/** Sizes icons usually come in; the next one up is asked for and the image
 * is scaled down, which stays sharp at fractional scales such as 125 %. */
const ICON_SIZES = [16, 20, 24, 32, 40, 48, 64, 96];
/** Largest request in bytes, well below the bridge's 64 KiB per message. */
const ICON_BATCH_BYTES = 32 * 1024;
/** `exe|path|size` to a `data:` URL. Only found icons are kept: a program
 * that starts later gets its icon the next time the list is drawn. */
const iconCache = new Map();
/** `exe|path|size` of icons asked for and not answered yet. */
const iconPending = new Map();

function iconKey(exe, path, size) {
  return `${exe.toLowerCase()}|${(path || '').toLowerCase()}|${size}`;
}

function iconSize() {
  const wanted = Math.ceil(ICON_SIZE * (window.devicePixelRatio || 1));
  return ICON_SIZES.find((s) => s >= wanted) ?? ICON_SIZES[ICON_SIZES.length - 1];
}

/** A placeholder of the icon's size; `loadIcons` puts the icon in. `path`
 * only for entries of the configuration: the host uses no other. */
function programIcon(exe, path) {
  const holder = el('span', { class: 'icon', 'aria-hidden': 'true' });
  holder.dataset.exe = exe;
  if (path) holder.dataset.path = path;
  return holder;
}

function showIcon(holder, url) {
  if (!url) return;
  holder.replaceChildren(el('img', { src: url, alt: '', width: ICON_SIZE, height: ICON_SIZE }));
  holder.classList.add('loaded');
}

/** Asks the host for `programs` at `size`, in requests below the size
 * limit; resolves when all answers are in the cache. */
async function fetchIcons(programs, size) {
  let batch = [];
  let bytes = 0;
  const send = async (list) => {
    // Not through `call`: icons change no state and report no errors.
    const reply = await request('icons', { programs: list, size });
    list.forEach((program, i) => {
      const url = reply.ok ? reply.data[i] : null;
      if (url) iconCache.set(iconKey(program.exe, program.path, size), url);
    });
  };
  const sends = [];
  for (const program of programs) {
    const cost = JSON.stringify(program).length + 1;
    if (batch.length === 256 || (batch.length > 0 && bytes + cost > ICON_BATCH_BYTES)) {
      sends.push(send(batch));
      batch = [];
      bytes = 0;
    }
    batch.push(program);
    bytes += cost;
  }
  if (batch.length > 0) sends.push(send(batch));
  await Promise.all(sends);
}

/** Fills every icon placeholder in `root`: known ones at once, the rest
 * with as few requests as possible. A missing icon leaves the placeholder. */
async function loadIcons(root) {
  const size = iconSize();
  const waiting = [];
  const programs = [];
  const asked = [];
  for (const holder of root.querySelectorAll('span.icon:not(.loaded)')) {
    const key = iconKey(holder.dataset.exe, holder.dataset.path, size);
    if (iconCache.has(key)) {
      showIcon(holder, iconCache.get(key));
      continue;
    }
    waiting.push(holder);
    if (!iconPending.has(key)) {
      const program = { exe: holder.dataset.exe };
      if (holder.dataset.path) program.path = holder.dataset.path;
      programs.push(program);
      asked.push(key);
    }
  }
  if (waiting.length === 0) return;
  if (programs.length > 0) {
    const done = fetchIcons(programs, size);
    for (const key of asked) iconPending.set(key, done);
    done.finally(() => { for (const key of asked) iconPending.delete(key); });
  }
  const keys = new Set(waiting.map((h) => iconKey(h.dataset.exe, h.dataset.path, size)));
  await Promise.all([...keys].map((key) => iconPending.get(key)).filter(Boolean));
  for (const holder of waiting) {
    if (holder.isConnected) {
      showIcon(holder, iconCache.get(iconKey(holder.dataset.exe, holder.dataset.path, size)));
    }
  }
}

/** Another scale (a monitor with other scaling, zoom): draw the lists again
 * so their icons are asked for in the new size. */
function watchScale() {
  const query = matchMedia(`(resolution: ${window.devicePixelRatio || 1}dppx)`);
  query.addEventListener('change', () => {
    if (state) render();
    watchScale();
  }, { once: true });
}

/** M5: an error of the last switch on its entry: code and a few words,
 * the whole error as tooltip. It goes when the tray no longer reports it
 * (resolved, or the next switch went well) at the next state refresh. */
function failureMarker(f) {
  return el('span', { class: 'failure', title: f.text, 'aria-label': f.text },
    el('span', { class: 'failure-mark', 'aria-hidden': 'true', text: '!' }),
    `${f.code} ${f.label}`);
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
  // U4: trigger programs, one per line, and the priority. Values the user
  // typed and has not saved (a refused Set, or typing while the state is
  // refreshed) stay until they are saved or another profile is shown.
  const triggers = $('triggers');
  const priority = $('priority');
  const draft = triggerDraft && triggerDraft.profile.toLowerCase() === profile.name.toLowerCase() ? triggerDraft : null;
  if (!draft) triggerDraft = null;
  if (document.activeElement !== triggers) triggers.value = draft ? draft.triggers : (profile.triggers || []).join('\n');
  if (document.activeElement !== priority) priority.value = draft ? draft.priority : String(profile.priority || 0);
  for (const field of [triggers, priority]) {
    if (draft && draft.invalid === field.id) field.setAttribute('aria-invalid', 'true');
    else field.removeAttribute('aria-invalid');
  }
  $('apply').disabled = state.applying;
  $('apply').textContent = state.applying ? 'Applying…' : 'Apply profile';

  const rows = profile.entries.map((e, index) => el('tr', { class: e.errors.length ? 'failed' : null },
    el('td', {}, el('div', { class: 'program' }, programIcon(e.exe, e.path),
      el('span', { class: 'names' }, el('span', { class: 'exe', text: e.exe }), e.path ? el('span', { class: 'path', text: e.path }) : null,
        ...e.errors.map(failureMarker)))),
    el('td', {}, el('span', { class: `action ${e.action}`, text: actionLabel[e.action] })),
    el('td', { class: 'opts', text: optionsText(e) }),
    el('td', {},
      el('button', { type: 'button', class: 'button small', 'aria-label': `Edit ${e.exe}`, onclick: () => editEntry(index), text: 'Edit' }),
      el('button', { type: 'button', class: 'button small danger', 'aria-label': `Remove ${e.exe}`, onclick: () => removeEntry(index), text: 'Remove' }))));
  $('entries').tBodies[0].replaceChildren(...rows);
  loadIcons($('entries'));
  $('entries').hidden = rows.length === 0;
  $('entries-empty').hidden = rows.length !== 0;
  renderUnassigned();
}

function renderUnassigned() {
  const rows = unassigned.map((u) => el('tr', {},
    el('td', {}, el('div', { class: 'program' }, programIcon(u.exe, null),
      el('span', { class: 'names' }, el('span', { class: 'exe', text: u.exe }), u.path ? el('span', { class: 'path', text: u.path }) : null))),
    el('td', {}, u.new ? el('span', { class: 'tag new', text: 'new' }) : 'earlier'),
    el('td', {},
      el('button', { type: 'button', class: 'button small', 'aria-label': `Add ${u.exe}`, onclick: () => addProgram(u.exe, null), text: 'Add' }),
      el('button', { type: 'button', class: 'button small', 'aria-label': `Dismiss ${u.exe}`, onclick: () => dismiss(u.exe), text: 'Dismiss' }))));
  $('unassigned').tBodies[0].replaceChildren(...rows);
  loadIcons($('unassigned'));
  $('unassigned').hidden = rows.length === 0;
  $('unassigned-empty').hidden = rows.length !== 0;
}

async function loadUnassigned() {
  const profile = current();
  unassigned = [];
  if (profile) {
    const reply = await call('unassigned', { profile: profile.name }, { quiet: true });
    // Another profile was selected meanwhile: its own request draws it.
    if (current()?.name !== profile.name) return;
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

/** M4: protected programs are refused, warn-list programs need a yes.
 * Why a program was not added goes to `problems` when given (M2 drops). */
async function addProgram(exe, path, problems = null) {
  const check = await call('check_program', { exe });
  if (!check.ok) {
    problems?.push(check.error);
    return false;
  }
  let confirmed = false;
  if (check.data.protection === 'hard') {
    $('add-error').textContent = check.data.message;
    notice(check.data.message);
    problems?.push(check.data.message);
    return false;
  }
  if (check.data.protection === 'warn') {
    confirmed = await confirmDialog('Warn list', check.data.message, 'Add anyway');
    if (!confirmed) {
      problems?.push(`${exe} was not confirmed`);
      return false;
    }
  }
  const reply = await call('add_entry', { profile: current().name, exe, path, confirmed });
  if (reply.ok) await loadUnassigned();
  else {
    $('add-error').textContent = reply.error;
    problems?.push(reply.error);
  }
  return reply.ok;
}

// Drag and drop (M2) ---------------------------------------------------------

/** The promise of the drop being handled (the test driver waits for it). */
let dropping = null;

function hasFiles(event) {
  return !!event.dataTransfer && [...event.dataTransfer.types].includes('Files');
}

/** Files can be dropped while a profile is shown and no dialog other than
 * `Add a program` is open. */
function canDrop() {
  return !!state && !!current() && !document.querySelector('dialog[open]:not(#add-dialog)');
}

// The page takes every drag and drop, so the WebView never opens a dropped
// file (the host cancels such a navigation as well).
addEventListener('dragover', (event) => {
  event.preventDefault();
  const ok = hasFiles(event) && canDrop();
  event.dataTransfer.dropEffect = ok ? 'copy' : 'none';
  $('detail').classList.toggle('dropping', ok);
});
addEventListener('dragleave', (event) => {
  if (!event.relatedTarget) $('detail').classList.remove('dropping');
});
addEventListener('drop', (event) => {
  event.preventDefault();
  $('detail').classList.remove('dropping');
  if (!hasFiles(event) || !canDrop() || event.dataTransfer.files.length === 0) return;
  dropping = dropFiles(event.dataTransfer.files);
});

/** Adds the dropped files to the selected profile as `Choose executable…`
 * would, one after the other; refused files are named in the status line. */
async function dropFiles(files) {
  const profile = current().name;
  localStatus = null;
  const reply = await requestWithFiles('drop_files', {}, files);
  if (!reply.ok) localStatus = { text: reply.error, isError: true };
  if (reply.state) adopt(reply.state);
  else if (!reply.ok) setStatus(reply.error, true);
  if (!reply.ok) return reply;
  const problems = [...reply.data.refused];
  let added = 0;
  for (const [index, file] of reply.data.files.entries()) {
    if (current()?.name !== profile) {
      // Another profile is shown now: the rest is not added anywhere.
      for (const rest of reply.data.files.slice(index)) {
        problems.push(`${rest.exe} was not added: another profile was selected`);
      }
      break;
    }
    if (await addProgram(file.exe, file.path, problems)) added++;
  }
  if (added > 0 && $('add-dialog').open) $('add-dialog').close();
  // The summary goes into the host's status line, so a refresh keeps it.
  await call('drop_done', { profile, added, not_added: problems });
  // With refusals the summary stays red, also after a refresh.
  if (problems.length > 0) notice($('status').textContent, true);
  return reply;
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
    el('td', {}, el('div', { class: 'program' }, programIcon(p.exe, null), el('span', { class: 'exe', text: p.exe }))),
    el('td', { class: 'num', text: megabytes(p.private_bytes) }),
    el('td', { class: `protection ${p.protection}`, text: protectionText[p.protection] }),
    el('td', {}, el('button', {
      type: 'button', class: 'button small', disabled: p.protection === 'hard',
      'aria-label': `Add ${p.exe}`,
      onclick: async () => { if (await addProgram(p.exe, null)) $('add-dialog').close(); },
      text: 'Add',
    }))));
  $('running').tBodies[0].replaceChildren(...rows);
  loadIcons($('running'));
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

/** U4: what was typed into the trigger fields and not saved yet. */
function keepTriggerDraft(invalid = null) {
  triggerDraft = { profile: current().name, triggers: $('triggers').value, priority: $('priority').value, invalid };
}
$('triggers').addEventListener('input', () => keepTriggerDraft());
$('priority').addEventListener('input', () => keepTriggerDraft());

$('triggers-form').addEventListener('submit', async (event) => {
  event.preventDefault();
  const priority = Number($('priority').value.trim() || '0');
  if (!Number.isInteger(priority)) {
    keepTriggerDraft('priority');
    $('priority').setAttribute('aria-invalid', 'true');
    notice('The priority is a whole number.');
    return;
  }
  const triggers = $('triggers').value.split(/\r?\n/).map((t) => t.trim()).filter(Boolean);
  // Kept while the request runs: the reply re-renders the fields.
  keepTriggerDraft();
  const reply = await call('set_triggers', { profile: current().name, triggers, priority });
  if (reply.ok) {
    triggerDraft = null;
    renderDetail();
  } else {
    keepTriggerDraft('triggers');
    $('triggers').setAttribute('aria-invalid', 'true');
  }
});

$('add-program').addEventListener('click', openAdd);
$('refresh-unassigned').addEventListener('click', loadUnassigned);
$('autostart').addEventListener('change', (event) => call('set_autostart', { on: event.target.checked }));
$('update-mode').addEventListener('change', (event) => call('set_update_mode', { mode: event.target.value }));
$('check-updates').addEventListener('click', () => call('check_updates'));
$('install-update').addEventListener('click', () => call('install_update'));

/** M5: back in the window, the state is asked for again, so markers of
 * errors that resolved meanwhile go (and the active profile is current).
 * Not while a dialog is open, at most every two seconds. */
let lastRefresh = 0;
addEventListener('focus', async () => {
  if (!state || document.querySelector('dialog[open]') || Date.now() - lastRefresh < 2000) return;
  lastRefresh = Date.now();
  const reply = await request('get_state');
  if (reply.ok && !document.querySelector('dialog[open]')) adopt(reply.state);
});

onEvent(async (event) => {
  if (event.event === 'applied') {
    // The end of a switch the user started: its result is the news now.
    // An update check may also run by itself, so it keeps the message.
    localStatus = null;
    adopt(event.state);
    await loadUnassigned();
  }
  if (event.event === 'update_checked') adopt(event.state);
});

// Test driver: runs bridge requests in order (kaltstellen gui --drive).
globalThis.kaltstellenDrive = async (steps) => {
  const results = [];
  for (const [index, step] of steps.entries()) {
    const { type, ...fields } = step;
    if (type === 'drop_real') {
      // M2: the host drops the step's files from disk through DevTools
      // (Input.dispatchDragEvent) onto the middle of the profile.
      const box = $('detail').getBoundingClientRect();
      dropping = null;
      notify('drive_drop', { step: index, x: Math.round(box.left + box.width / 2), y: Math.round(box.top + box.height / 2) });
      const deadline = Date.now() + 20000;
      while (!dropping && Date.now() < deadline) await new Promise((resolve) => setTimeout(resolve, 50));
      const reply = dropping ? await dropping : { ok: false, error: 'no drop arrived', state };
      results.push({ type, ok: reply.ok, error: reply.error || null, status: $('status').textContent, data: reply.data ?? null, state });
      continue;
    }
    if (type === 'drop') {
      // M2: a drop event as Explorer's would arrive, with files made up in
      // the page (they have names, no paths), through the real handler.
      const transfer = new DataTransfer();
      for (const name of fields.names) transfer.items.add(new File(['MZ'], name));
      dropping = null;
      $('entries').dispatchEvent(new DragEvent('drop', { dataTransfer: transfer, bubbles: true, cancelable: true }));
      const reply = dropping ? await dropping : { ok: false, error: 'the drop was not taken', state };
      results.push({ type, ok: reply.ok, error: reply.error || null, status: $('status').textContent, data: reply.data ?? null, state: reply.state || state });
      continue;
    }
    const reply = await request(type, fields);
    let event = null;
    if (type === 'apply' && reply.ok) {
      event = await new Promise((resolve) => onEvent((e) => { if (e.event === 'applied') resolve(e); }));
    }
    adopt((event || reply).state);
    const after = (event || reply).state;
    results.push({ type, ok: reply.ok, error: reply.error || null, status: after.status, data: reply.data ?? null, state: after });
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
  watchScale();
  const reply = await request('get_state');
  adopt(reply.state);
  await loadUnassigned();
  await openView(params.get('view'));
  await settle();
  notify('ready');
}

start();
