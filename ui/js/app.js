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
/** `profile` (a profile and its programs) or `settings` (M6). */
let view = 'profile';
/** M6: the program defaults are being edited and not saved yet. */
let defaultsDirty = false;

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

const AUTOMATION_HELP = 'Applies a profile when one of its trigger programs starts, and goes back when the last one has ended. Off pauses all triggers.';

function render() {
  $('active-badge').replaceChildren('Active: ', el('strong', { text: state.active }));
  $('autostart-row').hidden = state.autostart === null;
  $('autostart').checked = !!state.autostart;
  // U4: moved here from the tray menu; without triggers it does nothing yet.
  const automation = state.automation || { on: true, triggers: false };
  // An older tray that did not take a change keeps its own until it restarts.
  const hint = automation.note
    || (automation.triggers ? '' : 'No profile has triggers yet; set them under Triggers of a profile.');
  // The header and the Settings view show the same switch.
  for (const id of ['automation', 'automation-setting']) {
    $(id).checked = automation.on;
    $(id).disabled = !automation.triggers;
  }
  $('automation-wrap').classList.toggle('unused', !automation.triggers);
  $('automation-row').classList.toggle('disabled', !automation.triggers);
  $('automation-wrap').title = hint;
  $('automation-hint').textContent = hint;
  $('automation-setting-hint').textContent = hint || AUTOMATION_HELP;
  $('learn-setting').checked = state.learn !== false;
  $('notify-new-setting').checked = state.notify_new !== false;
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
  const pill = $('update-pill');
  pill.textContent = u.checking ? 'Checking…' : u.available ? 'Update available' : u.error ? 'Check failed' : u.last_check ? 'Up to date' : 'Not checked';
  pill.classList.toggle('warn', !!u.available || !!u.error);
  $('update-tag').hidden = !u.available;
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
  for (const radio of document.querySelectorAll('input[name="update-mode"]')) {
    radio.checked = radio.value === u.mode;
    radio.disabled = u.mode_locked;
  }
  $('row-mode').classList.toggle('disabled', u.mode_locked);
  $('update-locked').hidden = !u.mode_locked;
  $('check-updates').disabled = u.checking;
  const install = $('install-update');
  install.hidden = !u.available || !!u.blocked;
  install.textContent = u.available ? `Install ${u.available}` : 'Install';
}

/** M4: the optional background helper (service state, asked on demand). */
const HELPER_STATES = {
  not_installed: ['Not installed', 'Not installed. Everything works without it.'],
  stopped: ['Stopped', 'Installed, but the service is stopped.'],
  starting: ['Starting', 'The service is starting.'],
  running: ['Running', 'Installed and running.'],
  stopping: ['Stopping', 'The service is stopping.'],
  other: ['Other state', 'The service is in a state this window does not know.'],
  unknown: ['Unknown', 'The state of the service could not be read.'],
};

function renderHelper(info) {
  const [pill, text] = HELPER_STATES[info.state] || HELPER_STATES.unknown;
  const installed = !['not_installed', 'unknown'].includes(info.state);
  $('helper-pill').textContent = pill;
  $('helper-pill').classList.toggle('warn', info.state === 'unknown' || info.state === 'stopped' || (installed && !info.answers));
  const parts = [text];
  if (info.answers) parts.push(`Answers on its pipe${info.version ? `, version ${info.version}` : ''}.`);
  else if (installed) parts.push('It does not answer on its pipe.');
  parts.push(info.roles.length ? `Roles switched on: ${info.roles.join(', ')}.` : '');
  $('helper-state').textContent = parts.filter(Boolean).join(' ');
  $('helper-notes').replaceChildren(...info.notes.map((note) => el('li', { text: note })));
  $('helper-install').textContent = installed ? 'Update' : 'Install';
  // Unknown may still be an installed service, so Remove stays reachable.
  $('helper-uninstall').hidden = info.state === 'not_installed';
  // Back from a started step: the buttons work again.
  $('helper-install').disabled = false;
  $('helper-uninstall').disabled = false;
}

/** The host reads in the background and answers with a `helper_status`
 * event; a request while one runs is folded into it. */
async function loadHelper() {
  const reply = await request('helper_status');
  if (!reply.ok) renderHelper({ state: 'unknown', answers: false, version: null, roles: [], notes: [reply.error] });
}

/** One console at a time: the buttons wait until Refresh (or the next
 * time the window is entered) reads the state again. */
async function runHelperStep(type) {
  $('helper-install').disabled = true;
  $('helper-uninstall').disabled = true;
  const reply = await call(type);
  if (!reply.ok) {
    $('helper-install').disabled = false;
    $('helper-uninstall').disabled = false;
  }
}

function renderProfiles() {
  const list = $('profile-list');
  list.replaceChildren(...state.profiles.map((profile) => {
    const isSelected = view === 'profile' && profile === current();
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
  view = 'profile';
  suggestions = [];
  forgetAbout();
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
  const send = async (list) => {
    // Not through `call`: icons change no state and report no errors.
    const reply = await request('icons', { programs: list, size });
    list.forEach((program, i) => {
      const url = reply.ok ? reply.data[i] : null;
      if (url) iconCache.set(iconKey(program.exe, program.path, size), url);
    });
  };
  await Promise.all(batches(programs).map(send));
}

/** `programs` split into lists of at most 256 and below the size limit
 * (`icons`, `program_info`). */
function batches(programs) {
  const lists = [];
  let batch = [];
  let bytes = 0;
  for (const program of programs) {
    const cost = JSON.stringify(program).length + 1;
    if (batch.length === 256 || (batch.length > 0 && bytes + cost > ICON_BATCH_BYTES)) {
      lists.push(batch);
      batch = [];
      bytes = 0;
    }
    batch.push(program);
    bytes += cost;
  }
  if (batch.length > 0) lists.push(batch);
  return lists;
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

// What a program is (M2, N1) -------------------------------------------------

/** `exe|path` to what the host knows. Only finished answers are kept: a
 * pending one is asked again on the `program_info` event, a program
 * without a readable file (`null`) the next time the list is drawn. They
 * are kept only until the state is refreshed (focus), another profile is
 * shown or `Add a program` opens: a running program is known by its name
 * here, and the host finds its file (and whether it changed) afresh. */
const aboutCache = new Map();

function forgetAbout() {
  aboutCache.clear();
}

function aboutKey(exe, path) {
  return `${exe.toLowerCase()}|${(path || '').toLowerCase()}`;
}

/** An empty line under the program's name; `loadAbout` fills it. `path`
 * only for entries of the configuration, as for icons. */
function programAbout(exe, path) {
  const holder = el('span', { class: 'about' });
  holder.dataset.exe = exe;
  if (path) holder.dataset.path = path;
  return holder;
}

/** The tooltip: every field, and what is verified. */
function aboutTitle(v) {
  const lines = [];
  if (v.description) lines.push(`Description: ${v.description}`);
  if (v.product) lines.push(`Product: ${v.product}`);
  if (v.company) lines.push(`Company: ${v.company}`);
  if (v.version) lines.push(`Version: ${v.version}`);
  const signature = {
    valid: `Signed by ${v.publisher || 'an unnamed publisher'}${v.catalog ? ' (through a Windows catalog)' : ''}; the signature is valid.`,
    invalid: `The file has a signature, but it is not valid: ${v.problem}.`,
    unverified: `The file has a signature, but whether it is valid cannot be decided from what this computer has: ${v.problem}. That is not a sign that the file was changed.`,
    unsigned: 'The file is not signed.',
    unknown: `Signature not checked: ${v.problem}.`,
    pending: 'Checking the signature…',
  }[v.signature];
  if (signature) lines.push(signature);
  if (v.description || v.product || v.company) lines.push('Description, product and company are what the file says about itself; only a valid signature names a verified publisher.');
  return lines.join('\n');
}

/** Description (else product), the company where no valid signature names
 * a publisher, then the signature: the publisher with a check mark, a
 * warning for a broken signature, a discreet "unsigned". */
function showAbout(holder, v) {
  if (!v) {
    holder.classList.add('loaded');
    return;
  }
  const parts = [];
  let text = v.description || v.product || '';
  // Not while the signature is checked: the line would change twice.
  if (v.signature !== 'valid' && v.signature !== 'pending' && v.company) {
    if (!text) text = v.company;
    else if (!text.includes(v.company)) text = `${text} · ${v.company}`;
  }
  if (text) parts.push(text);
  if (v.signature === 'valid') parts.push(el('span', { class: 'publisher', text: `✓ ${v.publisher || 'signed'}` }));
  if (v.signature === 'invalid') parts.push(el('span', { class: 'sig-invalid', text: '⚠ signature not valid' }));
  // Neutral, not the warning colour: offline or unknown roots are no sign
  // of tampering.
  if (v.signature === 'unverified') parts.push(el('span', { class: 'sig-none', text: 'signature not verifiable here' }));
  if (v.signature === 'unsigned') parts.push(el('span', { class: 'sig-none', text: 'unsigned' }));
  const children = [];
  parts.forEach((part, i) => {
    if (i > 0) children.push(' · ');
    children.push(part);
  });
  holder.replaceChildren(...children);
  holder.title = aboutTitle(v);
  if (!v.pending) holder.classList.add('loaded');
}

/** Asks the host about `programs` in requests below the size limit;
 * resolves with `exe|path` to the answer. */
async function fetchAbout(programs) {
  const answers = new Map();
  const send = async (list) => {
    const reply = await request('program_info', { programs: list });
    list.forEach((program, i) => {
      const view = reply.ok ? reply.data[i] : null;
      answers.set(aboutKey(program.exe, program.path), view);
      if (view && !view.pending) aboutCache.set(aboutKey(program.exe, program.path), view);
    });
  };
  await Promise.all(batches(programs).map(send));
  return answers;
}

/** Fills every program line in `root` that is not final yet: known ones at
 * once, the rest with one round of requests. Pending ones stay open for
 * the next `program_info` event. */
async function loadAbout(root) {
  const holders = [...root.querySelectorAll('span.about:not(.loaded)')];
  const programs = [];
  const asked = new Set();
  for (const holder of holders) {
    const key = aboutKey(holder.dataset.exe, holder.dataset.path);
    if (aboutCache.has(key)) {
      showAbout(holder, aboutCache.get(key));
    } else if (!asked.has(key)) {
      asked.add(key);
      const program = { exe: holder.dataset.exe };
      if (holder.dataset.path) program.path = holder.dataset.path;
      programs.push(program);
    }
  }
  if (programs.length === 0) return;
  const answers = await fetchAbout(programs);
  for (const holder of holders) {
    const key = aboutKey(holder.dataset.exe, holder.dataset.path);
    if (holder.isConnected && !holder.classList.contains('loaded') && answers.has(key)) {
      showAbout(holder, answers.get(key));
    }
  }
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
  $('settings').hidden = view !== 'settings';
  $('empty').hidden = !!profile || view === 'settings';
  $('profile').hidden = !profile || view === 'settings';
  $('open-settings').setAttribute('aria-current', String(view === 'settings'));
  if (view === 'settings') renderSettings();
  if (!profile || view === 'settings') return;
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
  $('watch').checked = !!profile.watch;
  $('apply').disabled = state.applying;
  $('preview-apply').disabled = state.applying;
  $('apply').textContent = state.applying ? 'Applying…' : 'Apply profile';

  // Marked rows and the row in use belong to this profile.
  if (entryProfile !== profile.name.toLowerCase()) {
    entryProfile = profile.name.toLowerCase();
    entrySel.clear();
    entryLast = null;
    clearUndo();
  }
  const keys = new Set(profile.entries.map(entryKey));
  for (const key of [...entrySel]) if (!keys.has(key)) entrySel.delete(key);
  // A refresh must not take the focus from the control being used.
  const active = document.activeElement;
  const inTable = active && $('entries').contains(active) ? active : null;
  const focusAt = inTable && inTable.closest('tr[data-key]') ? {
    key: inTable.closest('tr[data-key]').dataset.key,
    selector: inTable.dataset.a ? `[data-a="${inTable.dataset.a}"]`
      : inTable.dataset.o ? `[data-o="${inTable.dataset.o}"]`
      : inTable.dataset.x ? `[data-x="${inTable.dataset.x}"]` : null,
  } : null;
  entryRemember(profile.entries);
  const rows = profile.entries.map((e, index) => entryRow(e, index));
  $('entries').tBodies[0].replaceChildren(...rows);
  if (focusAt) entryFocus(focusAt.key, focusAt.selector);
  renderEntryBulk(profile);
  loadIcons($('entries'));
  loadAbout($('entries'));
  $('entries').hidden = rows.length === 0;
  $('entries-empty').hidden = rows.length !== 0;
  renderUnassigned();
}

// Editing the programs of a profile (fast rows) ------------------------------

/** Rows of the table are keyed by program and path, which stay the same when
 * other rows are removed (indexes do not). */
const entryKey = (e) => `${e.exe.toLowerCase()}|${(e.path || '').toLowerCase()}`;
const entryActions = [['ignore', 'Ignore'], ['suspend', 'Freeze'], ['stop', 'Stop'], ['start', 'Start']];
const entrySel = new Set();
let entryProfile = null;
let entryLast = null;
let undoTimer = null;
let undoRemoved = null;

/** What a switch of the action would otherwise forget: `Trim` of a frozen
 * entry and `Restart on neutral` of a stopped one, by key. */
const entryMemory = new Map();
function entryRemember(entries) {
  for (const e of entries) {
    const memory = entryMemory.get(entryKey(e)) || {};
    if (e.action === 'suspend') memory.trim = e.trim ?? state.default_trim;
    if (e.action === 'stop') memory.restart = e.restart_on_neutral;
    entryMemory.set(entryKey(e), memory);
  }
}

const entryTouches = (action) => action === 'stop' || action === 'suspend';
const entryNeedsConfirm = (e, action) => e.protection === 'warn' && entryTouches(action) && !e.confirmed;

/** The whole entry as `update_entry` takes it, with `change` applied. */
function entryPayload(e, change = {}) {
  const next = {
    exe: e.exe,
    path: e.path || null,
    action: e.action,
    args: e.args,
    cwd: e.cwd || null,
    on_timeout: e.on_timeout,
    restart_on_neutral: e.restart_on_neutral,
    trim: e.trim,
    confirmed: e.confirmed,
    ...change,
  };
  const memory = entryMemory.get(entryKey(e)) || {};
  if (next.action === 'stop' && e.action !== 'stop' && change.restart_on_neutral === undefined) next.restart_on_neutral = !!memory.restart;
  if (next.action === 'suspend' && e.action !== 'suspend' && change.trim === undefined) next.trim = memory.trim ?? null;
  next.restart_on_neutral = next.action === 'stop' && next.restart_on_neutral;
  next.trim = next.action === 'suspend' ? (next.trim ?? state.default_trim) : null;
  return next;
}

function entryChips(e) {
  const chip = (option, label, on, title) => el('button', {
    type: 'button', class: 'chip', 'data-o': option, 'aria-pressed': String(on), title,
    'aria-label': `${label}, ${e.exe}`, text: label,
    onclick: () => (e.confirmed && option === 'confirmed' ? null : entryToggle(entryKey(e), option)),
  });
  const chips = [];
  if (e.action === 'suspend') chips.push(chip('trim', 'Trim', e.trim ?? state.default_trim, 'Move its memory out of RAM while frozen'));
  if (e.action === 'stop') {
    chips.push(chip('keep', 'Keep if it hangs', e.on_timeout === 'keep', 'Leave it running and report when it does not close in time'));
    chips.push(chip('restart', 'Restart on neutral', e.restart_on_neutral, 'Start it again when Neutral is applied'));
  }
  if (e.action === 'start' && e.args.length) chips.push(el('span', { class: 'chip static', title: 'Change in Details', text: `Arguments: ${e.args.join(' ')}` }));
  if (e.action === 'start' && e.cwd) chips.push(el('span', { class: 'chip static', title: 'Change in Details', text: `In ${e.cwd}` }));
  if (e.protection === 'warn' && entryTouches(e.action)) {
    const confirm = chip('confirmed', e.confirmed ? 'Confirmed' : 'Confirm', e.confirmed, 'On the warn list: kaltstellen touches it only after you confirm');
    if (e.confirmed) confirm.setAttribute('aria-disabled', 'true');
    chips.push(confirm);
  }
  return chips.length ? chips : [el('span', { class: 'hint', text: '–' })];
}

function entryRow(e, index) {
  const key = entryKey(e);
  const mark = el('input', { type: 'checkbox', 'aria-label': `Select ${e.exe}` });
  mark.checked = entrySel.has(key);
  mark.addEventListener('click', (event) => entryMark(key, mark.checked, event.shiftKey));
  const segment = el('span', { class: 'seg', role: 'group', 'aria-label': `Action for ${e.exe}` },
    ...entryActions.map(([value, label], i) => el('button', {
      type: 'button', 'data-a': value, 'aria-pressed': String(value === e.action),
      title: value === 'start' && !e.path ? 'Takes the path and arguments from the running program' : `${label} (key ${i + 1})`,
      'aria-label': `${label}, ${e.exe}`, text: label,
      onclick: () => entrySet([key], { action: value }),
    })));
  return el('tr', { 'data-key': key, tabindex: '0', class: [e.errors.length ? 'failed' : '', entrySel.has(key) ? 'selected' : ''].join(' ').trim() || null },
    el('td', { class: 'mark' }, mark),
    el('td', {}, el('div', { class: 'program' }, programIcon(e.exe, e.path),
      el('span', { class: 'names' }, el('span', { class: 'exe', text: e.exe }), programAbout(e.exe, e.path), e.path ? el('span', { class: 'path', text: e.path }) : null,
        e.protection === 'warn' ? el('span', { class: 'protection warn', text: 'warn list' }) : null,
        ...e.errors.map(failureMarker)))),
    el('td', {}, segment),
    el('td', {}, el('span', { class: 'chips' }, ...entryChips(e))),
    el('td', {},
      el('button', { type: 'button', class: 'button small', 'data-x': 'details', 'aria-label': `Edit ${e.exe}`, onclick: () => editEntry(index), text: 'Details…' }),
      el('button', { type: 'button', class: 'button small danger', 'data-x': 'remove', 'aria-label': `Remove ${e.exe}`, onclick: () => entryRemove([key]), text: 'Remove' })));
}

function entryFocus(key, selector) {
  const row = [...$('entries').tBodies[0].rows].find((r) => r.dataset.key === key);
  if (!row) return;
  (selector && row.querySelector(selector) || row).focus({ preventScroll: true });
}

function entryMark(key, on, range) {
  const order = current().entries.map(entryKey);
  if (range && entryLast !== null && order.includes(entryLast)) {
    const [a, b] = [order.indexOf(entryLast), order.indexOf(key)].sort((x, y) => x - y);
    for (const k of order.slice(a, b + 1)) entrySel.add(k);
  } else if (on) entrySel.add(key);
  else entrySel.delete(key);
  entryLast = key;
  render();
  entryFocus(key, 'input');
}

function renderEntryBulk(profile) {
  const n = entrySel.size;
  $('entries-bulk').hidden = n === 0;
  $('entries-count').textContent = `${n} selected`;
  const all = $('entries-all');
  all.checked = n > 0 && n === profile.entries.length;
  all.indeterminate = n > 0 && n < profile.entries.length;
}

const entriesOf = (keys) => keys.map((k) => current().entries.find((e) => entryKey(e) === k)).filter(Boolean);

/** Sets the action or an option of the entries with `keys`, one saved change
 * each. Entries on the warn list that are not confirmed yet are asked first. */
async function entrySet(keys, change) {
  const profile = current();
  if (!profile) return;
  const name = profile.name;
  // Trim only matters for frozen entries.
  let todo = entriesOf(keys)
    .filter((e) => !(change.trim !== undefined && e.action !== 'suspend'));
  // A program that is to start needs its path; both it and the arguments
  // are read from the running program. One that does not run and has no
  // path cannot be set to start (nothing is known about it).
  const launches = new Map();
  let lackingText = null;
  if (change.action === 'start') {
    const lacking = [];
    for (const e of todo) {
      if (e.action === 'start') continue;
      const found = await detectLaunch(e);
      if (found) launches.set(entryKey(e), found);
      else if (!e.path) lacking.push(e);
    }
    if (lacking.length) {
      const one = lacking.length === 1;
      lackingText = (`${lacking.map((e) => e.exe).join(', ')} ${one ? 'does' : 'do'} not run, so ${one ? 'its' : 'their'} path cannot be read. Start ${one ? 'it' : 'them'} once, or set the path in Details.`);
      todo = todo.filter((e) => !lacking.includes(e));
    }
  }
  const needs = change.action ? todo.filter((e) => entryNeedsConfirm(e, change.action)) : [];
  const allowed = new Set();
  if (needs.length) {
    const verb = change.action === 'suspend' ? 'freeze' : 'close';
    const text = needs.length === 1
      ? `${needs[0].exe} is on the warn list (remote access, drivers, security or developer tools). Let kaltstellen ${verb} it?`
      : `${needs.map((e) => e.exe).join(', ')} are on the warn list (remote access, drivers, security or developer tools). Let kaltstellen ${verb} them?`;
    if (await confirmDialog('Warn list', text, 'Confirm')) for (const e of needs) allowed.add(entryKey(e));
  }
  for (const e0 of todo) {
    const key = entryKey(e0);
    if (needs.includes(e0) && !allowed.has(key)) continue;
    const here = current();
    if (!here || here.name !== name) return;
    const index = here.entries.findIndex((e) => entryKey(e) === key);
    if (index < 0) continue;
    let patch = allowed.has(key) ? { ...change, confirmed: true } : change;
    const launch = launches.get(key);
    if (launch) {
      const e = here.entries[index];
      patch = { ...patch, path: e.path || launch.path, args: e.args.length ? e.args : launch.args };
    }
    const reply = await call('update_entry', { profile: name, index, entry: entryPayload(here.entries[index], patch) });
    if (!reply.ok) return;
    // The path is part of the key: carry selection and memory over.
    if (launch && !e0.path) {
      const next = `${e0.exe.toLowerCase()}|${launch.path.toLowerCase()}`;
      if (entrySel.delete(key)) entrySel.add(next);
      if (entryMemory.has(key)) entryMemory.set(next, entryMemory.get(key));
      if (entryLast === key) entryLast = next;
    }
  }
  if (lackingText) notice(lackingText);
}

/** How the running program of entry `e` was started: `{path, args}`, or
 * `null` if it does not run or cannot be read. Only its own file counts when
 * the entry has a path. */
async function detectLaunch(e) {
  const reply = await call('launch_of', { exe: e.exe, path: e.path || null });
  return reply.ok && reply.data && reply.data.found ? reply.data : null;
}

/** The switch of one option of an entry (chips). */
function entryToggle(key, option) {
  const e = entriesOf([key])[0];
  if (!e) return;
  const change = {
    trim: { trim: !(e.trim ?? state.default_trim) },
    keep: { on_timeout: e.on_timeout === 'keep' ? 'kill' : 'keep' },
    restart: { restart_on_neutral: !e.restart_on_neutral },
    confirmed: { confirmed: true },
  }[option];
  return entrySet([key], change);
}

function clearUndo() {
  clearTimeout(undoTimer);
  undoRemoved = null;
  $('entries-undo').hidden = true;
}

/** Removes the entries with `keys`; the programs themselves are not changed.
 * They can be brought back for a few seconds instead of being asked first. */
async function entryRemove(keys) {
  const profile = current();
  if (!profile) return;
  const name = profile.name;
  const gone = entriesOf(keys);
  const removed = [];
  for (const e of [...gone].sort((a, b) => entryIndex(b) - entryIndex(a))) {
    const here = current();
    if (!here || here.name !== name) break;
    const index = here.entries.findIndex((x) => entryKey(x) === entryKey(e));
    if (index < 0) continue;
    const reply = await call('remove_entry', { profile: name, index });
    if (!reply.ok) break;
    removed.push(e);
    entrySel.delete(entryKey(e));
  }
  if (removed.length) {
    clearTimeout(undoTimer);
    undoRemoved = { profile: name, entries: removed.sort((a, b) => gone.indexOf(a) - gone.indexOf(b)) };
    $('entries-undo-text').textContent = removed.length === 1 ? `${removed[0].exe} removed from "${name}".` : `${removed.length} programs removed from "${name}".`;
    $('entries-undo').hidden = false;
    undoTimer = setTimeout(clearUndo, 10000);
    renderEntryBulk(current());
    // The list got shorter: keep the keyboard where it was.
    const rows = [...$('entries').tBodies[0].rows];
    if (rows.length) rows[Math.min(rows.length - 1, Math.max(0, entryRemovedAt(profile, removed[0])))].focus({ preventScroll: true });
  }
  await loadUnassigned();
}

const entryIndex = (e) => current().entries.findIndex((x) => entryKey(x) === entryKey(e));
const entryRemovedAt = (before, e) => before.entries.findIndex((x) => entryKey(x) === entryKey(e));

async function entryUndo() {
  const undo = undoRemoved;
  clearUndo();
  if (!undo || !current() || current().name !== undo.profile) return;
  for (const e of undo.entries) {
    const added = await call('add_entry', { profile: undo.profile, exe: e.exe, path: e.path || null, confirmed: e.protection === 'warn' });
    if (!added.ok) return;
    const here = current();
    const reply = await call('update_entry', { profile: undo.profile, index: here.entries.length - 1, entry: entryPayload(e) });
    if (!reply.ok) return;
  }
  await loadUnassigned();
}

$('entries-undo-button').addEventListener('click', entryUndo);
$('entries-all').addEventListener('change', (event) => {
  entrySel.clear();
  if (event.target.checked) for (const e of current().entries) entrySel.add(entryKey(e));
  render();
});
$('entries-clear').addEventListener('click', () => {
  entrySel.clear();
  render();
  const first = $('entries').tBodies[0].rows[0];
  if (first) first.focus({ preventScroll: true });
});
$('entries-bulk-actions').addEventListener('click', (event) => {
  const action = event.target.dataset && event.target.dataset.a;
  if (action) entrySet([...entrySel], { action });
});
$('entries-trim-on').addEventListener('click', () => entrySet([...entrySel], { trim: true }));
$('entries-trim-off').addEventListener('click', () => entrySet([...entrySel], { trim: false }));
$('entries-remove').addEventListener('click', () => entryRemove([...entrySel]));

/** Keyboard: Up and Down move between rows, 1 to 4 set the action of the
 * marked rows when this row is marked (else of this row), Space marks, Delete
 * removes. */
$('entries').addEventListener('keydown', (event) => {
  const rowNode = event.target.closest && event.target.closest('tr[data-key]');
  if (!rowNode || event.ctrlKey || event.altKey || event.metaKey) return;
  const nodes = [...$('entries').tBodies[0].rows];
  const at = nodes.indexOf(rowNode);
  // The marked rows when this one is among them, else only this row.
  const keys = entrySel.has(rowNode.dataset.key) ? [...entrySel] : [rowNode.dataset.key];
  if (event.key === 'ArrowDown' || event.key === 'ArrowUp') {
    event.preventDefault();
    nodes[Math.max(0, Math.min(nodes.length - 1, at + (event.key === 'ArrowDown' ? 1 : -1)))].focus();
  } else if (event.key === ' ' && event.target === rowNode) {
    event.preventDefault();
    entryMark(rowNode.dataset.key, !entrySel.has(rowNode.dataset.key), event.shiftKey);
    entryFocus(rowNode.dataset.key, null);
  } else if (/^[1-4]$/.test(event.key) && event.target.tagName !== 'INPUT') {
    event.preventDefault();
    entrySet(keys, { action: entryActions[Number(event.key) - 1][0] });
  } else if (event.key === 'Delete' && event.target.tagName !== 'INPUT') {
    event.preventDefault();
    entryRemove(keys);
  }
});

// Settings (M6) ---------------------------------------------------------------

const DEFAULT_FIELDS = ['d-action', 'd-trim', 'd-minimize', 'd-timeout'];

function defaultsFromState() {
  return {
    default_action: state.default_action,
    trim: state.default_trim,
    stop_timeout_s: state.stop_timeout_s,
    minimize_before_suspend: state.minimize_before_suspend,
  };
}

function defaultsFromForm() {
  return {
    default_action: $('d-action').value,
    trim: $('d-trim').checked,
    stop_timeout_s: Number($('d-timeout').value),
    minimize_before_suspend: $('d-minimize').checked,
  };
}

function fillDefaults() {
  const d = defaultsFromState();
  $('d-action').value = d.default_action;
  $('d-trim').checked = d.trim;
  $('d-minimize').checked = d.minimize_before_suspend;
  $('d-timeout').value = String(d.stop_timeout_s);
}

function markDefaults(dirty) {
  defaultsDirty = dirty;
  $('defaults-save').disabled = !dirty;
  $('defaults-reset').disabled = !dirty;
  $('defaults-state').textContent = dirty ? 'Unsaved changes.' : 'Saved. Applies from the next switch.';
  $('d-timeout').removeAttribute('aria-invalid');
}

function keyCaps(hotkey) {
  return el('span', { class: 'keys' }, ...hotkey.split('+').map((key) => el('kbd', { text: key })));
}

function renderSettings() {
  if (!defaultsDirty && !DEFAULT_FIELDS.some((id) => document.activeElement === $(id))) {
    fillDefaults();
    markDefaults(false);
  }
  const rows = state.profiles.map((p) => el('tr', {},
    el('td', {}, el('strong', { text: p.name }), p.active ? ' ' : null, p.active ? el('span', { class: 'tag', text: 'active' }) : null),
    el('td', p.hotkey ? {} : { class: 'opts' }, p.hotkey ? keyCaps(p.hotkey) : 'none'),
    el('td', { class: p.triggers.length ? 'mono' : 'opts', text: p.triggers.length ? p.triggers.join(', ') : 'none' }),
    el('td', { class: 'num', text: String(p.priority || 0) }),
    el('td', {}, el('button', { type: 'button', class: 'button small', 'aria-label': `Open profile ${p.name}`, onclick: () => select(p.name), text: 'Open profile' }))));
  $('settings-profiles').tBodies[0].replaceChildren(...rows);
  const where = state.locations || {};
  $('file-config').textContent = state.config_file;
  $('file-settings').textContent = where.settings_file || '';
  $('file-log').textContent = where.log_file || '';
  $('open-log').disabled = !where.log_file;
  $('open-log-folder').disabled = !where.data_dir;
  $('forget-last').disabled = !where.last_profile;
  $('last-profile-hint').textContent = where.last_profile
    ? `Update mode, paused automatic switching and the profile that was active when you signed out (${where.last_profile}).`
    : 'Update mode, paused automatic switching and the profile that was active when you signed out (none saved).';
  $('about-version').textContent = `kaltstellen ${state.update.version}`;
}

/** Space above a section after choosing it (CSS pixels, as `--space-4`). */
const SECTION_GAP = 16;

// The window itself never scrolls; only the sidebar and the content area do.
// Script can still scroll a hidden viewport (focus(), scrollIntoView), and
// nothing would scroll it back, so put it back at once.
addEventListener('scroll', () => {
  const root = document.scrollingElement;
  if (root && (root.scrollTop || root.scrollLeft)) root.scrollTo(0, 0);
});

function showSettings(section = null) {
  view = 'settings';
  render();
  loadHelper();
  // Scroll the content area only. scrollIntoView would also scroll the
  // window itself (the body is hidden, but still scrollable by script),
  // which pushed the header out of sight with nothing to scroll back.
  const detail = $('detail');
  if (section) {
    const gap = $(section).getBoundingClientRect().top - detail.getBoundingClientRect().top;
    detail.scrollTo({ top: detail.scrollTop + gap - SECTION_GAP });
  } else detail.scrollTo({ top: 0 });
  for (const button of document.querySelectorAll('.settings-nav button')) {
    button.setAttribute('aria-current', String(button.dataset.section === (section || 'general')));
  }
}

$('open-settings').addEventListener('click', () => showSettings());
for (const button of document.querySelectorAll('.settings-nav button')) {
  button.addEventListener('click', () => showSettings(button.dataset.section));
}
$('automation-setting').addEventListener('change', (event) => call('set_automation', { on: event.target.checked }));
for (const radio of document.querySelectorAll('input[name="update-mode"]')) {
  radio.addEventListener('change', () => call('set_update_mode', { mode: radio.value }));
}
$('defaults-form').addEventListener('input', () => markDefaults(true));
$('defaults-reset').addEventListener('click', () => { fillDefaults(); markDefaults(false); });
$('defaults-form').addEventListener('submit', async (event) => {
  event.preventDefault();
  const reply = await call('set_defaults', defaultsFromForm());
  if (reply.ok) { fillDefaults(); markDefaults(false); }
  else {
    $('defaults-state').textContent = reply.error;
    $('d-timeout').setAttribute('aria-invalid', 'true');
  }
});
$('open-config-folder').addEventListener('click', () => call('open_location', { target: 'config_folder' }));
$('open-log').addEventListener('click', () => call('open_location', { target: 'log' }));
$('open-log-folder').addEventListener('click', () => call('open_location', { target: 'log_folder' }));
$('reload-config').addEventListener('click', () => call('reload_config'));
$('forget-last').addEventListener('click', () => call('forget_last_profile'));
$('watch').addEventListener('change', async (event) => {
  if (!current()) return;
  const on = event.target.checked;
  const reply = await call('set_watch', { profile: current().name, on });
  // A refused change leaves the switch as the file has it.
  if (!reply.ok) event.target.checked = !on;
});
$('learn-setting').addEventListener('change', async (event) => {
  await call('set_learn', { on: event.target.checked });
  await loadSuggestions();
});
$('notify-new-setting').addEventListener('change', async (event) => {
  await call('set_notify_new', { on: event.target.checked });
});
$('forget-learned').addEventListener('click', async () => {
  if (await confirmDialog('Forget what was learned', 'Delete the counters behind the suggestions? Entries you added stay.', 'Forget')) {
    await call('forget_learned');
    await loadSuggestions();
  }
});

// Suggestions from use (H5) ---------------------------------------------------

let suggestions = [];

const suggestionVerb = { stop: 'Stop', suspend: 'Freeze', start: 'Keep running (start)' };

function renderSuggestions() {
  const items = suggestions.map((s) => el('li', { class: 'suggestion' },
    el('div', {},
      el('span', { class: 'exe', text: s.exe }),
      el('span', { class: 'tag', text: suggestionVerb[s.action] || s.action }),
      s.tray && s.action !== 'start' ? trayTag() : null,
      el('p', { class: 'hint', text: s.reason })),
    el('div', { class: 'commands' },
      el('button', { type: 'button', class: 'button small', 'aria-label': `Add entry for ${s.exe}`, onclick: () => acceptSuggestion(s), text: s.entry === null ? 'Add entry' : 'Change entry' }),
      el('button', { type: 'button', class: 'button small', 'aria-label': `Not now for ${s.exe}`, onclick: () => hideSuggestion(s), text: 'Not now' }),
      el('button', { type: 'button', class: 'button small', 'aria-label': `Never suggest ${s.exe}`, onclick: () => neverSuggest(s), text: 'Never for this program' }))));
  $('suggestions').replaceChildren(...items);
  $('suggestions-title').textContent = `Suggestions (${suggestions.length})`;
  $('suggestions-card').hidden = suggestions.length === 0;
}

async function loadSuggestions() {
  const profile = current();
  suggestions = [];
  if (profile && state.learn) {
    const reply = await call('suggestions', { profile: profile.name }, { quiet: true });
    if (current()?.name !== profile.name) return;
    suggestions = reply.ok ? reply.data.map((s) => ({ ...s, profile: profile.name })).filter((s) => !hiddenSuggestions.has(`${profile.name}/${s.exe}`.toLowerCase())) : [];
  }
  renderSuggestions();
}

// "Not now" hides a suggestion until the window is opened again.
const hiddenSuggestions = new Set();

function hideSuggestion(s) {
  hiddenSuggestions.add(`${s.profile}/${s.exe}`.toLowerCase());
  suggestions = suggestions.filter((x) => x !== s);
  renderSuggestions();
}

let suggestionBusy = false;

async function neverSuggest(s) {
  if (suggestionBusy) return;
  suggestionBusy = true;
  try {
    await call('suggestion_done', { profile: s.profile, exe: s.exe, never: true });
    await loadSuggestions();
  } finally { suggestionBusy = false; }
}

async function acceptSuggestion(s) {
  if (suggestionBusy) return;
  suggestionBusy = true;
  try {
    const profile = profileNamed(s.profile);
    let reply;
    if (!profile) return;
    if (s.entry === null) {
      reply = await call('add_entries', { profile: profile.name, entries: [{ exe: s.exe, path: s.path, action: s.action, confirmed: false }] });
    } else {
      // The index came with the suggestion: check that it still is this program.
      const old = profile.entries[s.entry];
      if (!old || old.exe.toLowerCase() !== s.exe.toLowerCase()) {
        await loadSuggestions();
        return;
      }
      // Another action has other options: the entry starts with the defaults
      // of the new one, the program and its confirmation stay.
      reply = await call('update_entry', {
        profile: profile.name,
        index: s.entry,
        entry: { exe: old.exe, path: s.action === 'start' ? (s.path || old.path) : old.path, action: s.action, args: [], cwd: null, on_timeout: 'kill', restart_on_neutral: false, trim: null, confirmed: old.confirmed },
      });
    }
    if (reply.ok) await call('suggestion_done', { profile: profile.name, exe: s.exe, never: false });
    await loadSuggestions();
    await loadUnassigned();
  } finally { suggestionBusy = false; }
}

function renderUnassigned() {
  const rows = unassigned.map((u) => el('tr', {},
    el('td', {}, el('div', { class: 'program' }, programIcon(u.exe, null),
      el('span', { class: 'names' }, el('span', { class: 'exe', text: u.exe }), u.tray ? trayTag() : null, programAbout(u.exe, null), u.path ? el('span', { class: 'path', text: u.path }) : null))),
    el('td', {}, u.started ? el('span', { class: 'tag new', title: 'Started while this profile was active', text: 'started' }) : u.new ? el('span', { class: 'tag new', text: 'new' }) : 'earlier'),
    el('td', {},
      el('button', { type: 'button', class: 'button small', 'aria-label': `Add ${u.exe}`, onclick: () => addProgram(u.exe, null), text: 'Add' }),
      el('button', { type: 'button', class: 'button small', 'aria-label': `Dismiss ${u.exe}`, onclick: () => dismiss(u.exe), text: 'Dismiss' }))));
  $('unassigned').tBodies[0].replaceChildren(...rows);
  loadIcons($('unassigned'));
  loadAbout($('unassigned'));
  $('unassigned').hidden = rows.length === 0;
  $('unassigned-empty').hidden = rows.length !== 0;
  const started = unassigned.filter((u) => u.started).length;
  $('unassigned-started').hidden = started === 0;
  $('unassigned-started').textContent = `${started} ${started === 1 ? 'program' : 'programs'} started while this profile was active and ${started === 1 ? 'has' : 'have'} no entry. Add or dismiss ${started === 1 ? 'it' : 'them'}.`;
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
  await loadSuggestions();
}


// Compare two profiles (H6) -------------------------------------------------

const differenceLabel = {
  only_a: 'only in {a}', only_b: 'only in {b}', action: 'different action', options: 'different options',
};
let comparison = null;
let showIdentical = false;

function sameProgram(entry, row) {
  if (entry.exe.toLowerCase() !== row.exe.toLowerCase()) return false;
  const norm = (path) => path.replaceAll('/', '\\').toLowerCase();
  if (entry.path && row.path) return norm(entry.path) === norm(row.path);
  return true;
}

function profileNamed(name) {
  return state.profiles.find((p) => p.name.toLowerCase() === name.toLowerCase());
}

function renderCompare() {
  const a = current().name;
  const b = $('compare-with').value;
  const label = (key) => differenceLabel[key].replace('{a}', a).replace('{b}', b);
  const rows = comparison.rows;
  const identical = rows.filter((r) => !r.difference).length;
  const count = (key) => rows.filter((r) => r.difference === key).length;
  $('compare-summary').textContent = `${count('only_a')} only in ${a}, ${count('only_b')} only in ${b}, ${count('action') + count('options')} differ, ${identical} identical.`;
  $('compare-col-a').textContent = `In ${a}`;
  $('compare-col-b').textContent = `In ${b}`;
  const shown = rows.filter((r) => r.difference || showIdentical);
  $('compare-table').tBodies[0].replaceChildren(...shown.map((row) => el('tr', {},
    el('td', {}, el('span', { class: 'exe', text: row.exe }), row.path ? el('span', { class: 'path', text: row.path }) : null),
    el('td', { class: 'diff-cell', text: row.a || '—' }),
    el('td', { class: 'diff-cell', text: row.b || '—' }),
    el('td', {}, row.difference ? el('span', { class: `diff-tag ${row.difference}`, text: label(row.difference) }) : 'same'),
    el('td', {},
      row.difference && row.difference !== 'only_a'
        ? el('button', { type: 'button', class: 'button small', onclick: () => takeOver(row), text: `Take over from ${b}` }) : null,
      row.difference === 'only_a'
        ? el('button', { type: 'button', class: 'button small', onclick: () => removeFromA(row), text: `Remove from ${a}` }) : null))));
  const meta = [];
  const m = comparison.meta;
  if (m.hotkey) meta.push(`hotkey ${m.hotkey.a || 'none'} in ${a}, ${m.hotkey.b || 'none'} in ${b}`);
  if (m.triggers) meta.push(`triggers ${m.triggers.a || 'none'} in ${a}, ${m.triggers.b || 'none'} in ${b}`);
  if (m.priority) meta.push(`priority ${m.priority.a} in ${a}, ${m.priority.b} in ${b}`);
  $('compare-meta').textContent = meta.length ? `Not the same: ${meta.join(' · ')}.` : '';
  const toggle = $('compare-identical');
  toggle.hidden = identical === 0;
  toggle.textContent = showIdentical ? 'Hide identical' : `Show identical (${identical})`;
}

async function loadCompare() {
  $('compare-error').textContent = '';
  const reply = await call('compare', { a: current().name, b: $('compare-with').value }, { quiet: true });
  if (!reply.ok) {
    $('compare-error').textContent = reply.error;
    return;
  }
  comparison = reply.data;
  renderCompare();
}

async function openCompare() {
  const here = current();
  const others = state.profiles.filter((p) => p !== here);
  if (!here || others.length === 0) {
    notice('There is no other profile to compare with.', false);
    return;
  }
  $('compare-a').textContent = here.name;
  $('compare-with').replaceChildren(...others.map((p) => el('option', { value: p.name, text: p.name })));
  showIdentical = false;
  openDialog($('compare-dialog'));
  await loadCompare();
}

/** Copies the entry of the other profile into this one, or replaces the
 * entry it has (the same calls as the entry dialog). */
async function takeOver(row) {
  const here = current();
  const other = profileNamed($('compare-with').value);
  const from = other.entries.find((e) => sameProgram(e, row));
  if (!from) return;
  const edit = {
    exe: from.exe, path: from.path, action: from.action, args: from.args, cwd: from.cwd,
    on_timeout: from.on_timeout, restart_on_neutral: from.restart_on_neutral,
    trim: from.trim, confirmed: from.confirmed,
  };
  let index = here.entries.findIndex((e) => sameProgram(e, row));
  let reply;
  if (index >= 0) {
    reply = await call('update_entry', { profile: here.name, index, entry: edit });
  } else {
    reply = await call('add_entry', { profile: here.name, exe: from.exe, path: from.path, confirmed: from.confirmed || from.protection === 'warn' });
    if (reply.ok) {
      index = current().entries.length - 1;
      reply = await call('update_entry', { profile: here.name, index, entry: edit });
    }
  }
  if (!reply.ok) $('compare-error').textContent = reply.error;
  await loadCompare();
}

async function removeFromA(row) {
  const here = current();
  const index = here.entries.findIndex((e) => sameProgram(e, row));
  if (index < 0) return;
  const reply = await call('remove_entry', { profile: here.name, index });
  if (!reply.ok) $('compare-error').textContent = reply.error;
  await loadCompare();
}

$('compare').addEventListener('click', openCompare);
$('compare-with').addEventListener('change', loadCompare);
$('compare-identical').addEventListener('click', () => {
  showIdentical = !showIdentical;
  renderCompare();
});

// Profile from the current state (H1) --------------------------------------

const snapshotChoices = [
  ['none', 'Not in profile'], ['ignore', 'Leave as it is'], ['suspend', 'Freeze'],
  ['stop', 'Stop'], ['start', 'Keep running'],
];
let snap = null; // { mode, profile, data, choices: Map(key -> {action, confirmed}), load: Map(exe -> load), token }

/** H2: what a program costs, as text. */
function loadText(load) {
  if (!load) return 'measuring…';
  if (load.frozen) return 'frozen';
  const parts = [];
  if (load.cpu !== null) parts.push(`${(load.cpu * 100).toFixed(load.cpu < 0.1 ? 1 : 0)} % CPU`);
  if (load.io_per_s !== null) parts.push(`I/O ${load.io_per_s >= 1048576 ? `${(load.io_per_s / 1048576).toFixed(1)} MB/s` : `${Math.round(load.io_per_s / 1024)} KB/s`}`);
  const rate = (n) => (n >= 1048576 ? `${(n / 1048576).toFixed(1)} MB/s` : `${Math.round(n / 1024)} KB/s`);
  if (load.net_down_per_s != null) parts.push(`Network ↓ ${rate(load.net_down_per_s)} ↑ ${rate(load.net_up_per_s)}`);
  if (load.tcp || load.udp) parts.push(`${load.tcp} TCP, ${load.udp} UDP`);
  return parts.join(' · ') || 'idle';
}

const loadScore = (load) => (load && !load.frozen ? (load.cpu || 0) * 1e9 + (load.io_per_s || 0) : -1);

/** Measures all listed programs: two samples about 1.5 s apart (no timer,
 * only when the dialog opens or the owner asks). */
async function measureSnapshot() {
  // One measurement at a time: the bridge keeps one first sample.
  if (snap.busy) return;
  snap.busy = true;
  try {
    await runMeasurement();
  } finally {
    snap.busy = false;
  }
}

async function runMeasurement() {
  const token = {};
  snap.token = token;
  snap.load = new Map();
  $('snapshot-measuring').textContent = 'Measuring for 1.5 seconds…';
  renderSnapshot();
  const programs = [...new Set(snapRows().map((r) => r.exe))];
  await call('measure_start', {}, { quiet: true });
  await new Promise((resolve) => setTimeout(resolve, 1500));
  if (snap.token !== token) return;
  const reply = await call('measure', { programs }, { quiet: true });
  if (snap.token !== token) return;
  $('snapshot-measuring').textContent = reply.ok ? '' : `No measurement: ${reply.error}`;
  if (reply.ok) for (const row of reply.data.programs) snap.load.set(row.exe.toLowerCase(), row);
  renderSnapshot();
}

const snapActionLabel = { ignore: 'Leave as it is', suspend: 'Freeze', stop: 'Stop', start: 'Keep running' };

/** The tag for a program with a notification-area icon entry. Windows lists
 * it in the registry; whether the icon shows right now is not known. */
const TRAY_HELP = 'Has a tray icon entry. Stopping removes the icon; freezing leaves it standing, but the program no longer reacts to it.';
function trayTag() {
  return el('span', { class: 'diff-tag only_b', title: TRAY_HELP, text: 'Tray icon' });
}

/** H3: warnings from the open windows, as short texts. */
function snapWarnings(row) {
  const w = row.windows || { count: 0 };
  const list = [];
  if (w.count > 0) list.push(`${w.count} ${w.count === 1 ? 'window' : 'windows'} open`);
  if (w.dialog) list.push('Dialog open');
  if (w.unsaved) list.push('May have unsaved changes');
  if (w.hung) list.push('Not responding');
  return list;
}

/** With Stop, open windows or unsaved work make the entry keep the
 * program if it does not close (A2); the entry dialog can change it. */
function snapKeeps(row, action) {
  const w = row.windows || { count: 0 };
  return action === 'stop' && (w.count > 0 || w.dialog || w.unsaved);
}

/** Whether the list shows the row now: background programs and rows the
 * `Heavy only` filter hides are not touched by quick fills. */
function snapShown(row) {
  if (snap.data.running.includes(row) && row.background && !$('snapshot-background').checked) return false;
  if ($('snapshot-heavy').checked && !snap.load.get(row.exe.toLowerCase())?.heavy) return false;
  if ($('snapshot-tray').checked && !row.tray) return false;
  return true;
}

const snapKey = (row) => `${row.exe.toLowerCase()}|${(row.path || '').toLowerCase()}`;

/** What the owner can pick for a row: a program without a known path (an
 * elevated one, P7) can only be left as it is; `Keep running` needs a path. */
function snapOptions(row) {
  return snapshotChoices.filter(([value]) => {
    if (row.elevated) return value === 'none' || value === 'ignore';
    if (value === 'start') return !!row.path;
    return true;
  });
}

function snapRows() {
  return [...snap.data.running, ...snap.data.held, ...snap.data.seen];
}

function snapSetDefaults() {
  for (const row of snap.data.held) {
    if (!snap.choices.has(snapKey(row))) snap.choices.set(snapKey(row), { action: row.held, confirmed: row.protection === 'warn' });
  }
}

function snapEverything() {
  snap.choices.clear();
  const set = (row, action) => snap.choices.set(snapKey(row), { action, confirmed: false });
  // Only what the list shows: background programs stay out unless shown,
  // and the filters (Heavy only, Tray icon only) count. What kaltstellen
  // holds keeps its action either way.
  for (const row of snap.data.running) if (snapShown(row)) set(row, row.path && !row.elevated ? 'start' : 'ignore');
  for (const row of snap.data.held) snap.choices.set(snapKey(row), { action: row.held, confirmed: row.protection === 'warn' });
  // A warn-list program is not stopped without its own confirmation (P6).
  for (const row of snap.data.seen) if (row.protection !== 'warn' && snapShown(row)) set(row, 'stop');
}

/** Order of two rows by kind: known kinds by their plural name, unknown last. */
const snapKindOrder = (x, y) => (snapKindId(x) === 'unknown') - (snapKindId(y) === 'unknown')
  || (x.kind?.plural || '').localeCompare(y.kind?.plural || '');

/** H8: the kind of a row; a program of a kind nobody could name is `unknown`. */
const snapKindId = (row) => (row.kind && row.kind.known ? row.kind.id : 'unknown');

/** H8: one line per kind found among the rows the list shows, with one
 * choice for all of them. Nothing is remembered as a rule: it only sets the
 * rows once, like the bulk bar. Warn-list programs are not changed for
 * Freeze or Stop (they need their own confirmation, P6). */
/** Rows a Freeze or Stop for a whole kind leaves alone. */
const snapLocked = (row) => row.protection === 'warn' || row.elevated;

function snapRenderKinds() {
  const panel = $('snapshot-kinds');
  if (panel.hidden) return;
  const rows = new Map(snapRows().map((r) => [snapKey(r), r]));
  const kinds = new Map();
  for (const key of snap.order) {
    const row = rows.get(key);
    if (!row) continue;
    const id = snapKindId(row);
    if (!kinds.has(id)) kinds.set(id, { id, plural: row.kind && row.kind.plural ? row.kind.plural : 'programs', rows: [] });
    kinds.get(id).rows.push(row);
  }
  const had = panel.contains(document.activeElement) && document.activeElement.dataset ? document.activeElement : null;
  const hadAt = had ? { line: [...panel.querySelectorAll('.kind-line')].indexOf(had.closest('.kind-line')), a: had.dataset.a } : null;
  const lines = [...kinds.values()].sort((a, b) => (a.id === 'unknown') - (b.id === 'unknown') || a.plural.localeCompare(b.plural));
  panel.replaceChildren(
    el('p', { class: 'hint', text: 'One choice for every program of a kind that the list shows. It sets the rows once; nothing is remembered, and you can change each row afterwards.' }),
    ...lines.map((kind) => el('div', { class: 'kind-line' },
      el('span', { class: 'kind-name' }, el('strong', { text: kind.id === 'unknown' ? `Unknown kind (${kind.rows.length})` : `All ${kind.plural} (${kind.rows.length})` }),
        el('span', { class: 'hint', text: kind.rows.map((r) => r.exe).join(', ') }),
        kind.rows.some(snapLocked) ? el('span', { class: 'hint', text: `${kind.rows.filter(snapLocked).length} of them keep their choice for Freeze or Stop (warn list or higher integrity).` }) : null),
      el('span', { class: 'seg', role: 'group', 'aria-label': `Choice for all ${kind.plural}` },
        ...snapshotChoices.map(([value, label]) => el('button', {
          type: 'button', 'data-a': value, 'aria-label': `${label}, all ${kind.plural}`, text: snapShort[value],
          onclick: () => {
            const keys = kind.rows
              .filter((r) => !(r.protection === 'warn' && (value === 'suspend' || value === 'stop')))
              .map(snapKey);
            snapSet(keys, value, null);
          },
        }))))));
  if (hadAt && hadAt.line >= 0) panel.querySelectorAll('.kind-line')[hadAt.line]?.querySelector(`button[data-a="${hadAt.a}"]`)?.focus();
  if (lines.length === 0) panel.append(el('p', { class: 'hint', text: 'No programs are shown.' }));
}

const snapShort = { none: 'Not', ignore: 'As is', suspend: 'Freeze', stop: 'Stop', start: 'Run' };
const snapKeyNumber = (value) => snapshotChoices.findIndex(([v]) => v === value) + 1;

/** Sets one action for rows by key. A row that cannot have it (an elevated
 * program cannot be frozen, `Keep running` needs a path) keeps what it has,
 * and a warn-list program is never confirmed here (P6). */
function snapSet(keys, action, focusKey) {
  const rows = new Map(snapRows().map((r) => [snapKey(r), r]));
  for (const key of keys) {
    const row = rows.get(key);
    if (!row || !snapOptions(row).some(([v]) => v === action)) continue;
    snap.choices.set(key, { action, confirmed: false });
  }
  snapRender(focusKey, action);
}

function snapMark(key, on, range) {
  if (range && snap.last && snap.order.includes(snap.last)) {
    const [a, b] = [snap.order.indexOf(snap.last), snap.order.indexOf(key)].sort((x, y) => x - y);
    for (const k of snap.order.slice(a, b + 1)) snap.sel.add(k);
  } else if (on) snap.sel.add(key);
  else snap.sel.delete(key);
  snap.last = key;
  snapRender(key, 'mark');
}

/** Renders again and puts the keyboard focus back where it was (or on the
 * row and button named), so keyboard use survives every re-render. */
function snapRender(focusKey, action) {
  renderSnapshot(focusKey, action);
}

function snapBulkBar() {
  const bar = $('snapshot-bulk');
  // What a filter hides is not selected any more.
  for (const key of [...snap.sel]) if (!snap.order.includes(key)) snap.sel.delete(key);
  const count = snap.sel.size;
  bar.hidden = count === 0;
  $('snapshot-bulk-count').textContent = `${count} selected`;
}

function snapTable(title, rows, note) {
  if ($('snapshot-heavy').checked) rows = rows.filter((r) => snap.load.get(r.exe.toLowerCase())?.heavy);
  if ($('snapshot-tray').checked) rows = rows.filter((r) => r.tray);
  if ($('snapshot-sort').checked || $('snapshot-sortkind').checked) {
    // Kind first when asked for, then the load; unknown kinds come last.
    const byKind = $('snapshot-sortkind').checked;
    const load = (r) => ($('snapshot-sort').checked ? loadScore(snap.load.get(r.exe.toLowerCase())) : 0);
    rows = [...rows].sort((x, y) => (byKind ? snapKindOrder(x, y) : 0) || load(y) - load(x));
  }
  const wrap = el('section', { class: 'snap-group' }, el('h3', { text: `${title} (${rows.length})` }));
  if (note) wrap.append(el('p', { class: 'hint', text: note }));
  if (rows.length === 0) return wrap;
  const body = el('tbody');
  for (const row of rows) {
    const key = snapKey(row);
    const choice = snap.choices.get(key) || { action: 'none', confirmed: false };
    const options = snapOptions(row);
    const current = options.some(([v]) => v === choice.action) ? choice.action : 'none';
    const confirm = el('input', { type: 'checkbox', 'aria-label': `I confirm for ${row.exe}` });
    confirm.checked = choice.confirmed;
    const confirmCell = el('label', { class: 'inline' }, confirm, ' I confirm');
    const needsConfirm = (action) => row.protection === 'warn' && (action === 'suspend' || action === 'stop');
    confirmCell.hidden = !needsConfirm(current);
    confirm.addEventListener('change', () => {
      snap.choices.set(key, { action: current, confirmed: confirm.checked });
      snapUpdateOk();
    });
    const segment = el('span', { class: row.suggestion ? 'seg suggested' : 'seg', role: 'group', 'aria-label': `Action for ${row.exe}` },
      ...options.map(([value, label]) => el('button', {
        type: 'button', 'data-a': value, 'aria-pressed': String(value === current), 'aria-label': `${label}, ${row.exe}`, title: `${label} (key ${snapKeyNumber(value)})`,
        text: snapShort[value],
        onclick: () => snapSet([key], value, key),
      })));
    const mark = el('input', { type: 'checkbox', 'aria-label': `Select ${row.exe}` });
    mark.checked = snap.sel.has(key);
    mark.addEventListener('click', (event) => snapMark(key, mark.checked, event.shiftKey));
    snap.order.push(key);
    body.append(el('tr', { 'data-key': key, tabindex: '0', class: snap.sel.has(key) ? 'selected' : null },
      el('td', { class: 'mark' }, mark),
      el('td', {}, el('div', { class: 'program' }, programIcon(row.exe, null),
        el('span', { class: 'names' }, el('span', { class: 'exe', text: row.exe }),
          row.path ? el('span', { class: 'path', text: row.path }) : el('span', { class: 'path', text: row.elevated ? 'higher integrity, path not readable' : 'path not known' }),
          row.protection === 'warn' ? el('span', { class: 'protection warn', text: 'warn list' }) : null,
          row.tray ? trayTag() : null,
          row.kind && row.kind.known ? el('span', { class: 'hint', text: `probably ${row.kind.label}` }) : null,
          row.suggestion ? el('span', { class: 'hint', text: `Suggested: ${snapActionLabel[row.suggestion.action]} (${row.suggestion.reason})` }) : null,
          ...snapWarnings(row).map((text) => el('span', { class: 'diff-tag only_a', text }))))),
      el('td', { class: 'num', text: row.private_bytes ? megabytes(row.private_bytes) : '' }),
      snapLoadCell(row),
      el('td', {}, segment, confirmCell)));
  }
  wrap.append(el('div', {}, el('table', { class: 'table' },
    el('thead', {}, el('tr', {}, el('th', { scope: 'col', class: 'mark' }), el('th', { scope: 'col', text: 'Program' }), el('th', { scope: 'col', class: 'num', text: 'Memory' }), el('th', { scope: 'col', text: 'Load' }), el('th', { scope: 'col', text: 'In the profile' }))),
    body)));
  return wrap;
}

function snapLoadCell(row) {
  // Programs that do not run have no load to show.
  if (!snap.data.running.includes(row) && !snap.data.held.includes(row)) return el('td');
  const load = snap.load.get(row.exe.toLowerCase());
  return el('td', { class: 'diff-cell' }, loadText(load),
    load && load.heavy ? el('span', { class: 'diff-tag only_b', text: ` Heavy: ${load.heavy}` }) : null);
}

function snapChosen() {
  const rows = snapRows();
  const chosen = rows.filter((r) => (snap.choices.get(snapKey(r)) || { action: 'none' }).action !== 'none');
  const names = new Map();
  for (const r of chosen) names.set(r.exe.toLowerCase(), (names.get(r.exe.toLowerCase()) || 0) + 1);
  return chosen.map((row) => {
    const choice = snap.choices.get(snapKey(row));
    // The same name from different folders needs the path to tell them
    // apart (C2); `start` always needs it (C6).
    const withPath = choice.action === 'start' || names.get(row.exe.toLowerCase()) > 1;
    return {
      exe: row.exe, path: withPath && row.path ? row.path : null, action: choice.action, confirmed: choice.confirmed,
      on_timeout: snapKeeps(row, choice.action) ? 'keep' : null, warn: row.protection === 'warn',
    };
  });
}

function snapUpdateOk() {
  const count = snapChosen().length;
  $('snapshot-ok').textContent = snap.mode === 'new' ? `Create profile (${count})` : `Add ${count} ${count === 1 ? 'entry' : 'entries'}`;
  $('snapshot-ok').disabled = count === 0 && snap.mode === 'update';
}

function renderSnapshot(focusKey, action) {
  snap.order = [];
  const box = $('snapshot-dialog');
  const top = box.scrollTop;
  const active = document.activeElement;
  const inRow = active && $('snapshot-groups').contains(active) && active.closest ? active.closest('tr[data-key]') : null;
  if (!focusKey && inRow) {
    focusKey = inRow.dataset.key;
    if (active.tagName === 'BUTTON') action = active.dataset.a;
    else if (active.tagName === 'INPUT' && active.getAttribute('aria-label')?.startsWith('Select ')) action = 'mark';
  }
  const showBackground = $('snapshot-background').checked;
  const running = snap.data.running.filter((r) => showBackground || !r.background);
  const hidden = snap.data.running.length - running.length;
  $('snapshot-groups').replaceChildren(
    snapTable('Running now', running, hidden > 0 ? `${hidden} background ${hidden === 1 ? 'program is' : 'programs are'} hidden.` : ''),
    snapTable('Held by kaltstellen', snap.data.held, snap.data.held.length ? 'Programs kaltstellen froze or stopped and that nobody has resumed.' : ''),
    snapTable('Not running, seen earlier', snap.data.seen, ''));
  loadIcons($('snapshot-groups'));
  snapBulkBar();
  snapRenderKinds();
  box.scrollTop = top;
  if (focusKey) {
    const node = [...$('snapshot-groups').querySelectorAll('tr[data-key]')].find((r) => r.dataset.key === focusKey);
    const target = !node ? null : action === 'mark' ? node.querySelector('input[aria-label^="Select "]') : action ? node.querySelector(`button[data-a="${action}"]`) : null;
    (target || node)?.focus();
  }
  const existing = $('snapshot-existing');
  existing.hidden = !(snap.mode === 'update' && snap.data.existing > 0) && !snap.note;
  if (snap.note && snap.mode === 'new') existing.textContent = snap.note;
  else existing.textContent = `${snap.data.existing} ${snap.data.existing === 1 ? 'program already has an entry' : 'programs already have an entry'} in this profile and ${snap.data.existing === 1 ? 'is' : 'are'} not listed. Use Compare to change entries.`;
  snapUpdateOk();
}

async function loadSnapshot() {
  $('snapshot-error').textContent = '';
  const reply = await call('snapshot', { mode: snap.mode, profile: snap.profile }, { quiet: true });
  if (!reply.ok) {
    $('snapshot-error').textContent = reply.error;
    return false;
  }
  snap.data = reply.data;
  snapSetDefaults();
  renderSnapshot();
  return true;
}

async function openSnapshot(mode, template) {
  snap = { mode, profile: mode === 'update' ? current().name : null, data: null, choices: new Map(), load: new Map(), token: null, sel: new Set(), order: [], last: null };
  $('snapshot-title').textContent = mode === 'new' ? 'New profile from the current state' : `Update ${current().name} from the current state`;
  $('snapshot-name-row').hidden = mode !== 'new';
  $('snapshot-name').value = template ? template.name : '';
  $('snapshot-background').checked = false;
  $('snapshot-measuring').textContent = '';
  $('snapshot-heavy').checked = false;
  $('snapshot-tray').checked = false;
  $('snapshot-sort').checked = false;
  $('snapshot-sortkind').checked = false;
  $('snapshot-kinds').hidden = true;
  $('snapshot-kinds-toggle').setAttribute('aria-expanded', 'false');
  if (!(await loadSnapshot())) return;
  if (template) {
    const count = snapApplyTemplate(template);
    $('snapshot-title').textContent = `New profile from the ${template.name} template`;
    snap.note = `The ${template.name} template set ${count} ${count === 1 ? 'row' : 'rows'} (${snap.touched} to freeze or stop, the rest left as they are). Check every row; nothing is created before you confirm.`;
    renderSnapshot();
  }
  openDialog($('snapshot-dialog'));
  measureSnapshot();
}

const TEMPLATE_VERB = { suspend: 'Freeze', stop: 'Stop', ignore: 'Leave as it is' };

/** H7: the rows a template sets. Only programs of a known kind that the
 * template has a rule for, never a warn-list program (P6) and never what
 * kaltstellen holds; the choice has to be one the row can have. Nothing is
 * written here: the owner still sees and confirms every row. */
function snapTemplateRows(template) {
  const rules = new Map(template.rules.map((r) => [r.kind, r.action]));
  const out = [];
  for (const row of [...snap.data.running, ...snap.data.seen]) {
    if (!row.kind || !row.kind.known || row.protection !== 'none' || row.elevated) continue;
    const action = rules.get(row.kind.id);
    if (!action) continue;
    if (!snapOptions(row).some(([v]) => v === action)) continue;
    out.push([row, action]);
  }
  return out;
}

function snapApplyTemplate(template) {
  let count = 0;
  let touched = 0;
  for (const [row, action] of snapTemplateRows(template)) {
    if (snap.data.running.includes(row) && row.background) continue;
    snap.choices.set(snapKey(row), { action, confirmed: false });
    count += 1;
    if (action !== 'ignore') touched += 1;
  }
  snap.touched = touched;
  return count;
}
$('snapshot-dialog').addEventListener('close', () => { if (snap) snap.token = null; });

$('snapshot-update').addEventListener('click', () => openSnapshot('update'));
$('snapshot-refresh').addEventListener('click', async () => { if (await loadSnapshot()) measureSnapshot(); });
$('snapshot-measure').addEventListener('click', measureSnapshot);
$('snapshot-heavy').addEventListener('change', renderSnapshot);
$('snapshot-tray').addEventListener('change', renderSnapshot);
$('snapshot-sort').addEventListener('change', renderSnapshot);
$('snapshot-sortkind').addEventListener('change', () => renderSnapshot());
$('snapshot-kinds-toggle').addEventListener('click', () => {
  const panel = $('snapshot-kinds');
  panel.hidden = !panel.hidden;
  $('snapshot-kinds-toggle').setAttribute('aria-expanded', String(!panel.hidden));
  renderSnapshot();
});
$('snapshot-background').addEventListener('change', renderSnapshot);
$('snapshot-suggestions').addEventListener('click', () => {
  // Only rows that have a suggestion and no choice yet; a suggestion is
  // a pre-selected choice, never an entry the owner did not see.
  for (const row of snapRows()) {
    if (!row.suggestion || row.protection !== 'none' || !snapShown(row)) continue;
    const key = snapKey(row);
    const now = snap.choices.get(key);
    if (now && now.action !== 'none') continue;
    if (!snapOptions(row).some(([v]) => v === row.suggestion.action)) continue;
    snap.choices.set(key, { action: row.suggestion.action, confirmed: false });
  }
  renderSnapshot();
});

$('snapshot-bulk-actions').addEventListener('click', (event) => {
  const action = event.target.dataset && event.target.dataset.a;
  if (action) snapSet([...snap.sel].filter((k) => snap.order.includes(k)), action, null);
});
$('snapshot-unmark').addEventListener('click', () => {
  snap.sel.clear();
  snapRender();
});
$('snapshot-all').addEventListener('click', () => {
  for (const key of snap.order) snap.sel.add(key);
  snapRender();
});

/** Keyboard: Up and Down move between rows, 1 to 5 set the action of the
 * selected rows (or of this row when none is selected), Space selects. */
$('snapshot-groups').addEventListener('keydown', (event) => {
  const rowNode = event.target.closest && event.target.closest('tr[data-key]');
  if (!rowNode || event.ctrlKey || event.altKey || event.metaKey) return;
  const nodes = [...$('snapshot-groups').querySelectorAll('tr[data-key]')];
  const at = nodes.indexOf(rowNode);
  if (event.key === 'ArrowDown' || event.key === 'ArrowUp') {
    event.preventDefault();
    nodes[Math.max(0, Math.min(nodes.length - 1, at + (event.key === 'ArrowDown' ? 1 : -1)))].focus();
  } else if (event.key === ' ' && event.target === rowNode) {
    event.preventDefault();
    snapMark(rowNode.dataset.key, !snap.sel.has(rowNode.dataset.key), event.shiftKey);
  } else if (/^[1-5]$/.test(event.key)) {
    const action = snapshotChoices[Number(event.key) - 1][0];
    const keys = snap.sel.size ? [...snap.sel].filter((k) => snap.order.includes(k)) : [rowNode.dataset.key];
    event.preventDefault();
    snapSet(keys, action, rowNode.dataset.key);
  }
});
$('snapshot-clear').addEventListener('click', () => {
  snap.note = null;
  snap.choices.clear();
  renderSnapshot();
});
$('snapshot-everything').addEventListener('click', () => {
  snap.note = null;
  snapEverything();
  renderSnapshot();
});
$('snapshot-form').addEventListener('submit', async (event) => {
  event.preventDefault();
  const entries = snapChosen();
  const unconfirmed = entries.find((e) => e.warn && (e.action === 'stop' || e.action === 'suspend') && !e.confirmed);
  if (unconfirmed) {
    $('snapshot-error').textContent = `${unconfirmed.exe} is on the warn list: tick "I confirm" or choose another action.`;
    return;
  }
  const payload = entries.map(({ exe, path, action, confirmed, on_timeout }) => ({ exe, path, action, confirmed, on_timeout }));
  let reply;
  if (snap.mode === 'new') {
    const name = $('snapshot-name').value.trim();
    if (!name) {
      $('snapshot-error').textContent = 'The profile needs a name.';
      return;
    }
    reply = await call('create_profile_from', { name, entries: payload });
    if (reply.ok) {
      $('snapshot-dialog').close();
      await select(name);
    }
  } else {
    reply = await call('add_entries', { profile: snap.profile, entries: payload });
    if (reply.ok) $('snapshot-dialog').close();
  }
  if (!reply.ok) $('snapshot-error').textContent = reply.error;
});

// Preview a switch (H4) -----------------------------------------------------

let previewed = null;
let previewAll = false;

const stepLabel = { resume: 'Resume', stop: 'Stop', suspend: 'Freeze', start: 'Start' };

function renderPreview() {
  const items = previewed.items;
  const will = items.filter((i) => i.outcome === 'will').length;
  const problems = previewed.problems;
  const parts = [`${will} ${will === 1 ? 'program' : 'programs'} would be changed`];
  if (previewed.private_bytes > 0) parts.push(`${megabytes(previewed.private_bytes)} of private memory in what is stopped or frozen`);
  parts.push(problems.length ? `${problems.length} ${problems.length === 1 ? 'problem' : 'problems'} expected` : 'no problems expected');
  $('preview-summary').textContent = `${parts.join(', ')}.`;
  $('preview-problems').replaceChildren(...problems.map((text) => el('p', { class: 'error', text })));
  const shown = items.filter((i) => i.outcome !== 'no_change' || previewAll);
  const order = { problem: 0, will: 1, no_change: 2 };
  const sorted = shown.map((item, at) => ({ item, at })).sort((x, y) => order[x.item.outcome] - order[y.item.outcome] || x.at - y.at);
  $('preview-table').tBodies[0].replaceChildren(...sorted.map(({ item }) => el('tr', { class: item.outcome === 'problem' ? 'failed' : null },
    el('td', { text: stepLabel[item.step] || item.step }),
    el('td', {}, el('span', { class: 'exe', text: item.program })),
    el('td', { class: 'diff-cell', text: item.text }))));
  const quiet = items.length - items.filter((i) => i.outcome !== 'no_change').length;
  const toggle = $('preview-nochange');
  toggle.hidden = quiet === 0;
  toggle.textContent = previewAll ? 'Hide programs without a change' : `Show programs without a change (${quiet})`;
}

async function openPreview() {
  const name = current().name;
  $('preview-error').textContent = '';
  const reply = await call('preview', { profile: name }, { quiet: true });
  if (!reply.ok) {
    notice(reply.error, false);
    return;
  }
  previewed = reply.data;
  previewAll = false;
  $('preview-title').textContent = `Preview: switch to ${name}`;
  renderPreview();
  openDialog($('preview-dialog'));
}

$('preview').addEventListener('click', openPreview);
$('preview-nochange').addEventListener('click', () => {
  previewAll = !previewAll;
  renderPreview();
});
$('preview-apply').addEventListener('click', () => {
  $('preview-dialog').close();
  call('apply', { profile: previewed.profile });
});

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

$('e-action').addEventListener('change', () => {
  showOptions();
  // Switched to Start in the dialog: fill what is still empty.
  if ($('e-action').value === 'start' && (!$('e-path').value.trim() || !$('e-args').value.trim())) detectIntoForm(false);
});
$('e-detect').addEventListener('click', () => detectIntoForm(true));

/** Fills path and arguments of the dialog from the running program. Fields
 * that already have a value are kept unless `overwrite`. */
async function detectIntoForm(overwrite) {
  const exe = $('e-exe').value.trim();
  if (!exe) return;
  const found = await detectLaunch({ exe, path: $('e-path').value.trim() || null });
  if (!found) {
    if (overwrite) $('entry-error').textContent = `${exe} does not run (or cannot be read), so there is nothing to take over.`;
    return;
  }
  $('entry-error').textContent = '';
  if (overwrite || !$('e-path').value.trim()) $('e-path').value = found.path;
  if (overwrite || !$('e-args').value.trim()) $('e-args').value = found.args.join('\n');
  if (!found.args_known) $('entry-error').textContent = 'Its arguments cannot be read (the program is protected, elevated or has unusual arguments); only the path is taken over where it was empty.';
}
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
  return !!state && view === 'profile' && !!current() && !document.querySelector('dialog[open]:not(#add-dialog)');
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
  forgetAbout();
  $('add-error').textContent = '';
  const reply = await call('running');
  const running = reply.ok ? reply.data : [];
  const rows = running.map((p) => el('tr', {},
    el('td', {}, el('div', { class: 'program' }, programIcon(p.exe, null),
      el('span', { class: 'names' }, el('span', { class: 'exe', text: p.exe }), programAbout(p.exe, null),
        p.kind && p.kind.known ? el('span', { class: 'hint', text: `probably ${p.kind.label}` }) : null,
        p.suggestion ? el('span', { class: 'hint', text: `Suggested: ${snapActionLabel[p.suggestion.action]} (${p.suggestion.reason})` }) : null))),
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
  loadAbout($('running'));
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

$('new-profile').addEventListener('click', () => openDialog($('new-dialog')));
$('new-empty').addEventListener('click', () => {
  $('new-dialog').close();
  nameDialog('New profile', '', async (name) => {
    const reply = await call('create_profile', { name });
    if (reply.ok) await select(name.trim());
    return reply;
  });
});
$('new-template').addEventListener('click', async () => {
  $('new-dialog').close();
  $('template-error').textContent = '';
  const [templates, groups] = await Promise.all([
    call('templates', {}, { quiet: true }),
    call('snapshot', { mode: 'new' }, { quiet: true }),
  ]);
  if (!templates.ok) {
    notice(templates.error);
    return;
  }
  if (!groups.ok) $('template-error').textContent = groups.error;
  snap = { mode: 'new', profile: null, data: groups.ok ? groups.data : null, choices: new Map(), load: new Map(), token: null, sel: new Set(), order: [], last: null };
  $('template-list').replaceChildren(...templates.data.map((t) => {
    const words = t.rules.map((r) => `${r.label}: ${TEMPLATE_VERB[r.action]}`).join('; ');
    const found = snap.data ? snapTemplateRows(t).filter(([row, action]) => action !== 'ignore' && !(snap.data.running.includes(row) && row.background)).length : null;
    return el('section', { class: 'snap-group' },
      el('h3', { text: t.name }),
      el('p', { text: t.purpose }),
      el('p', { class: 'hint', text: words }),
      el('p', { class: 'hint', text: found === null ? 'The programs on this computer could not be read.' : `${found} ${found === 1 ? 'program' : 'programs'} found that this template would freeze or stop.` }),
      el('button', { type: 'button', class: 'button primary small', text: 'Use this template', onclick: () => {
        $('template-dialog').close();
        openSnapshot('new', t);
      } }));
  }));
  openDialog($('template-dialog'));
});
$('new-snapshot').addEventListener('click', () => {
  $('new-dialog').close();
  openSnapshot('new');
});

$('rename').addEventListener('click', () => {
  const from = current().name;
  nameDialog('Rename profile', from, async (to) => {
    // An unsaved trigger edit moves with the profile to its new name.
    if (triggerDraft && triggerDraft.profile.toLowerCase() === from.toLowerCase()) {
      triggerDraft.profile = to.trim();
    }
    const reply = await call('rename_profile', { from, to });
    if (!reply.ok && triggerDraft && triggerDraft.profile === to.trim()) triggerDraft.profile = from;
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
  const profile = current().name;
  const reply = await call('set_triggers', { profile, triggers, priority });
  // Another profile may be shown by now: its fields are not touched.
  if (current()?.name !== profile) return;
  if (reply.ok) {
    triggerDraft = null;
    renderDetail();
  } else {
    // The field the refusal is about.
    const field = /priority/i.test(reply.error || '') ? 'priority' : 'triggers';
    keepTriggerDraft(field);
    $(field).setAttribute('aria-invalid', 'true');
  }
});

$('add-program').addEventListener('click', openAdd);
$('refresh-unassigned').addEventListener('click', loadUnassigned);
$('autostart').addEventListener('change', (event) => call('set_autostart', { on: event.target.checked }));
$('automation').addEventListener('change', (event) => call('set_automation', { on: event.target.checked }));
$('check-updates').addEventListener('click', () => call('check_updates'));
$('install-update').addEventListener('click', () => call('install_update'));
$('helper-refresh').addEventListener('click', loadHelper);
$('helper-install').addEventListener('click', () => runHelperStep('helper_install'));
$('helper-uninstall').addEventListener('click', () => runHelperStep('helper_uninstall'));

/** M5: back in the window, the state is asked for again, so markers of
 * errors that resolved meanwhile go (and the active profile is current).
 * Not while a dialog is open, at most every two seconds. */
let lastRefresh = 0;
addEventListener('focus', async () => {
  if (!state || document.querySelector('dialog[open]') || Date.now() - lastRefresh < 2000) return;
  lastRefresh = Date.now();
  if (view === 'settings') loadHelper();
  const reply = await request('get_state');
  if (reply.ok && !document.querySelector('dialog[open]')) {
    forgetAbout();
    adopt(reply.state);
  }
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
  if (event.event === 'helper_status') renderHelper(event.data);
  // M2, N1: the host read more about programs; fill what was pending.
  if (event.event === 'program_info') {
    aboutEvents++;
    loadAbout(document.body);
  }
});

/** `program_info` events seen (for the test driver). */
let aboutEvents = 0;

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
    if (type === 'program_info') {
      // M2: asks again until nothing is pending (the host reads in the
      // background), at most 20 s; reports the events seen meanwhile.
      let reply = await request(type, fields);
      const deadline = Date.now() + 20000;
      while (reply.ok && reply.data.some((v) => v && v.pending) && Date.now() < deadline) {
        await new Promise((resolve) => setTimeout(resolve, 100));
        reply = await request(type, fields);
      }
      // The event may come just after the answer that was final.
      const until = Date.now() + 2000;
      while (aboutEvents === 0 && Date.now() < until) await new Promise((resolve) => setTimeout(resolve, 50));
      results.push({ type, ok: reply.ok, error: reply.error || null, status: state.status, data: reply.data ?? null, events: aboutEvents, state });
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

/** Screenshot views: ?view=entry|add|confirm|new|preview opens that dialog. */
async function openView(name) {
  const profile = current();
  if (name === 'entry' && profile && profile.entries.length) editEntry(Math.min(2, profile.entries.length - 1));
  if (name === 'add' && profile) await openAdd();
  if (name === 'confirm' && profile) confirmDialog('Warn list', 'anydesk.exe is a remote-access tool. Freezing or closing it can cut you off or break things. Add it anyway?', 'Add anyway');
  if (name === 'settings') showSettings();
  if (name === 'compare' && profile) await openCompare();
  if (name === 'preview' && profile) await openPreview();
  if (name === 'snapshot' && profile) await openSnapshot('new');
  if (name === 'new') nameDialog('New profile', 'Evening', async () => ({ ok: false, error: 'KS205: profile name "Evening" is used already' }));
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
