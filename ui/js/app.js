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
  $('apply').disabled = state.applying;
  $('preview-apply').disabled = state.applying;
  $('apply').textContent = state.applying ? 'Applying…' : 'Apply profile';

  const rows = profile.entries.map((e, index) => el('tr', { class: e.errors.length ? 'failed' : null },
    el('td', {}, el('div', { class: 'program' }, programIcon(e.exe, e.path),
      el('span', { class: 'names' }, el('span', { class: 'exe', text: e.exe }), programAbout(e.exe, e.path), e.path ? el('span', { class: 'path', text: e.path }) : null,
        ...e.errors.map(failureMarker)))),
    el('td', {}, el('span', { class: `action ${e.action}`, text: actionLabel[e.action] })),
    el('td', { class: 'opts', text: optionsText(e) }),
    el('td', {},
      el('button', { type: 'button', class: 'button small', 'aria-label': `Edit ${e.exe}`, onclick: () => editEntry(index), text: 'Edit' }),
      el('button', { type: 'button', class: 'button small danger', 'aria-label': `Remove ${e.exe}`, onclick: () => removeEntry(index), text: 'Remove' }))));
  $('entries').tBodies[0].replaceChildren(...rows);
  loadIcons($('entries'));
  loadAbout($('entries'));
  $('entries').hidden = rows.length === 0;
  $('entries-empty').hidden = rows.length !== 0;
  renderUnassigned();
}

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

function renderUnassigned() {
  const rows = unassigned.map((u) => el('tr', {},
    el('td', {}, el('div', { class: 'program' }, programIcon(u.exe, null),
      el('span', { class: 'names' }, el('span', { class: 'exe', text: u.exe }), programAbout(u.exe, null), u.path ? el('span', { class: 'path', text: u.path }) : null))),
    el('td', {}, u.new ? el('span', { class: 'tag new', text: 'new' }) : 'earlier'),
    el('td', {},
      el('button', { type: 'button', class: 'button small', 'aria-label': `Add ${u.exe}`, onclick: () => addProgram(u.exe, null), text: 'Add' }),
      el('button', { type: 'button', class: 'button small', 'aria-label': `Dismiss ${u.exe}`, onclick: () => dismiss(u.exe), text: 'Dismiss' }))));
  $('unassigned').tBodies[0].replaceChildren(...rows);
  loadIcons($('unassigned'));
  loadAbout($('unassigned'));
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
  if (load.tcp || load.udp) parts.push(`${load.tcp} TCP, ${load.udp} UDP`);
  return parts.join(' · ') || 'idle';
}

const loadScore = (load) => (load && !load.frozen ? (load.cpu || 0) * 1e9 + (load.io_per_s || 0) : -1);

/** Measures all listed programs: two samples about 1.5 s apart (no timer,
 * only when the dialog opens or the owner asks). */
async function measureSnapshot() {
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
  // Only what the list shows: background programs stay out unless shown.
  const shown = $('snapshot-background').checked;
  for (const row of snap.data.running) if (shown || !row.background) set(row, row.path && !row.elevated ? 'start' : 'ignore');
  for (const row of snap.data.held) snap.choices.set(snapKey(row), { action: row.held, confirmed: row.protection === 'warn' });
  // A warn-list program is not stopped without its own confirmation (P6).
  for (const row of snap.data.seen) if (row.protection !== 'warn') set(row, 'stop');
}

function snapTable(title, rows, note) {
  if ($('snapshot-heavy').checked) rows = rows.filter((r) => snap.load.get(r.exe.toLowerCase())?.heavy);
  if ($('snapshot-sort').checked) rows = [...rows].sort((a, b) => loadScore(snap.load.get(b.exe.toLowerCase())) - loadScore(snap.load.get(a.exe.toLowerCase())));
  const wrap = el('section', { class: 'snap-group' }, el('h3', { text: `${title} (${rows.length})` }));
  if (note) wrap.append(el('p', { class: 'hint', text: note }));
  if (rows.length === 0) return wrap;
  const body = el('tbody');
  for (const row of rows) {
    const key = snapKey(row);
    const choice = snap.choices.get(key) || { action: 'none', confirmed: false };
    const select = el('select', { 'aria-label': `Action for ${row.exe}` },
      ...snapOptions(row).map(([value, label]) => el('option', { value, text: label })));
    select.value = snapOptions(row).some(([v]) => v === choice.action) ? choice.action : 'none';
    const confirm = el('input', { type: 'checkbox', 'aria-label': `I confirm for ${row.exe}` });
    confirm.checked = choice.confirmed;
    const confirmCell = el('label', { class: 'inline' }, confirm, ' I confirm');
    const sync = () => {
      snap.choices.set(key, { action: select.value, confirmed: confirm.checked });
      confirmCell.hidden = !(row.protection === 'warn' && (select.value === 'suspend' || select.value === 'stop'));
      snapUpdateOk();
    };
    select.addEventListener('change', sync);
    confirm.addEventListener('change', sync);
    confirmCell.hidden = !(row.protection === 'warn' && (select.value === 'suspend' || select.value === 'stop'));
    body.append(el('tr', {},
      el('td', {}, el('div', { class: 'program' }, programIcon(row.exe, null),
        el('span', { class: 'names' }, el('span', { class: 'exe', text: row.exe }),
          row.path ? el('span', { class: 'path', text: row.path }) : el('span', { class: 'path', text: row.elevated ? 'higher integrity, path not readable' : 'path not known' }),
          row.protection === 'warn' ? el('span', { class: 'protection warn', text: 'warn list' }) : null))),
      el('td', { class: 'num', text: row.private_bytes ? megabytes(row.private_bytes) : '' }),
      snapLoadCell(row),
      el('td', {}, select, confirmCell)));
  }
  wrap.append(el('div', {}, el('table', { class: 'table' },
    el('thead', {}, el('tr', {}, el('th', { scope: 'col', text: 'Program' }), el('th', { scope: 'col', class: 'num', text: 'Memory' }), el('th', { scope: 'col', text: 'Load' }), el('th', { scope: 'col', text: 'In the profile' }))),
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
    return { exe: row.exe, path: withPath && row.path ? row.path : null, action: choice.action, confirmed: choice.confirmed, warn: row.protection === 'warn' };
  });
}

function snapUpdateOk() {
  const count = snapChosen().length;
  $('snapshot-ok').textContent = snap.mode === 'new' ? `Create profile (${count})` : `Add ${count} ${count === 1 ? 'entry' : 'entries'}`;
  $('snapshot-ok').disabled = count === 0 && snap.mode === 'update';
}

function renderSnapshot() {
  const showBackground = $('snapshot-background').checked;
  const running = snap.data.running.filter((r) => showBackground || !r.background);
  const hidden = snap.data.running.length - running.length;
  $('snapshot-groups').replaceChildren(
    snapTable('Running now', running, hidden > 0 ? `${hidden} background ${hidden === 1 ? 'program is' : 'programs are'} hidden.` : ''),
    snapTable('Held by kaltstellen', snap.data.held, snap.data.held.length ? 'Programs kaltstellen froze or stopped and that nobody has resumed.' : ''),
    snapTable('Not running, seen earlier', snap.data.seen, ''));
  loadIcons($('snapshot-groups'));
  const existing = $('snapshot-existing');
  existing.hidden = !(snap.mode === 'update' && snap.data.existing > 0);
  existing.textContent = `${snap.data.existing} ${snap.data.existing === 1 ? 'program already has an entry' : 'programs already have an entry'} in this profile and ${snap.data.existing === 1 ? 'is' : 'are'} not listed. Use Compare to change entries.`;
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

async function openSnapshot(mode) {
  snap = { mode, profile: mode === 'update' ? current().name : null, data: null, choices: new Map(), load: new Map(), token: null };
  $('snapshot-title').textContent = mode === 'new' ? 'New profile from the current state' : `Update ${current().name} from the current state`;
  $('snapshot-name-row').hidden = mode !== 'new';
  $('snapshot-name').value = '';
  $('snapshot-background').checked = false;
  $('snapshot-measuring').textContent = '';
  $('snapshot-heavy').checked = false;
  $('snapshot-sort').checked = false;
  if (!(await loadSnapshot())) return;
  openDialog($('snapshot-dialog'));
  measureSnapshot();
}
$('snapshot-dialog').addEventListener('close', () => { if (snap) snap.token = null; });

$('snapshot-update').addEventListener('click', () => openSnapshot('update'));
$('snapshot-refresh').addEventListener('click', async () => { if (await loadSnapshot()) measureSnapshot(); });
$('snapshot-measure').addEventListener('click', measureSnapshot);
$('snapshot-heavy').addEventListener('change', renderSnapshot);
$('snapshot-sort').addEventListener('change', renderSnapshot);
$('snapshot-background').addEventListener('change', renderSnapshot);
$('snapshot-clear').addEventListener('click', () => {
  snap.choices.clear();
  renderSnapshot();
});
$('snapshot-everything').addEventListener('click', () => {
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
  const payload = entries.map(({ exe, path, action, confirmed }) => ({ exe, path, action, confirmed }));
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
      el('span', { class: 'names' }, el('span', { class: 'exe', text: p.exe }), programAbout(p.exe, null)))),
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

/** M5: back in the window, the state is asked for again, so markers of
 * errors that resolved meanwhile go (and the active profile is current).
 * Not while a dialog is open, at most every two seconds. */
let lastRefresh = 0;
addEventListener('focus', async () => {
  if (!state || document.querySelector('dialog[open]') || Date.now() - lastRefresh < 2000) return;
  lastRefresh = Date.now();
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
