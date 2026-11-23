// Mock backend for design work in a normal browser. It answers the same
// messages as the Rust side (docs/gui-bridge.md) from sample data kept in
// memory. It is not embedded into the executable.

const PROTECTED = new Set(['explorer.exe', 'csrss.exe', 'dwm.exe', 'winlogon.exe', 'svchost.exe']);
const WARN = new Set(['anydesk.exe', 'teamviewer.exe', 'nvcontainer.exe', 'code.exe']);

function protectionOf(exe) {
  const name = exe.toLowerCase();
  if (PROTECTED.has(name)) return 'hard';
  if (WARN.has(name)) return 'warn';
  return 'none';
}

function entry(exe, action, extra = {}) {
  return {
    exe, path: null, action, args: [], cwd: null, on_timeout: 'kill',
    restart_on_neutral: false, trim: action === 'suspend' ? true : null,
    confirmed: false, protection: protectionOf(exe), errors: [], ...extra,
  };
}

const state = {
  config_file: 'C:\\Users\\you\\AppData\\Roaming\\kaltstellen\\config.toml',
  active: 'Flight',
  default_action: 'suspend',
  default_trim: true,
  autostart: false,
  automation: { on: true, triggers: true, note: null },
  applying: false,
  status: 'Mock backend: nothing is saved.',
  update: {
    version: '1.0.0', mode: 'auto', mode_locked: false, available: null,
    blocked: null, failed: false, retry_at: null, last_check: 1795000000, error: null, checking: false,
  },
  profiles: [
    {
      name: 'Flight', hotkey: 'Ctrl+Shift+Alt+F1', active: true, triggers: ['simulator.exe'], priority: 10,
      entries: [
        entry('browser.exe', 'suspend'),
        entry('mail.exe', 'suspend', { trim: false }),
        entry('chat.exe', 'stop', { restart_on_neutral: true, args: ['--minimized'] }),
        // M5: an error of the last switch on its entry.
        entry('tracker.exe', 'start', {
          path: 'C:\\Tools\\Tracker\\tracker.exe', args: ['--profile', 'flight'],
          errors: [{ code: 'KS110', label: 'start failed', text: 'KS110 [start] tracker.exe: C:\\Tools\\Tracker\\tracker.exe: The system cannot find the file specified.' }],
        }),
        entry('anydesk.exe', 'ignore'),
      ],
    },
    {
      name: 'Music production', hotkey: 'Ctrl+Shift+Alt+F2', active: false, triggers: [], priority: 0,
      entries: [
        entry('browser.exe', 'suspend'),
        entry('sync.exe', 'stop', { on_timeout: 'keep' }),
      ],
    },
    { name: 'Quiet evening', hotkey: null, active: false, triggers: [], priority: 0, entries: [] },
  ],
};

const running = [
  { exe: 'browser.exe', private_bytes: 1_340_000_000 },
  { exe: 'game-launcher.exe', private_bytes: 412_000_000 },
  { exe: 'mail.exe', private_bytes: 380_000_000 },
  { exe: 'explorer.exe', private_bytes: 160_000_000 },
  { exe: 'anydesk.exe', private_bytes: 48_000_000 },
  { exe: 'notes.exe', private_bytes: 35_500_000 },
].map((p) => ({ ...p, protection: protectionOf(p.exe) }));

const unassigned = {
  'Flight': [
    { exe: 'game-launcher.exe', path: 'C:\\Games\\Launcher\\game-launcher.exe', new: true },
    { exe: 'notes.exe', path: null, new: false },
  ],
};

/** M2, N1: what the sample programs are. */
const mockAbout = {
  'browser.exe': { description: 'Web Browser', product: 'Browser', company: 'Example Browser Corp', signature: 'valid', publisher: 'Example Browser Corp' },
  'mail.exe': { description: 'Mail', company: 'Mail Makers', signature: 'valid', publisher: 'Mail Makers Ltd', catalog: false },
  'chat.exe': { description: 'Chat client', company: 'Chatty', signature: 'invalid', problem: 'the file was changed after signing' },
  'sync.exe': { description: 'File sync', company: 'Sync Example Ltd', signature: 'unverified', problem: 'its root certificate is not known to this computer' },
  'explorer.exe': { description: 'Windows Explorer', company: 'Microsoft Corporation', signature: 'valid', publisher: 'Microsoft Windows', catalog: true },
  'anydesk.exe': { description: 'Remote desk', company: 'Remote Example GmbH', signature: 'valid', publisher: 'Remote Example GmbH' },
  'game-launcher.exe': { description: 'Game Launcher', company: 'Games Inc.', signature: 'unsigned' },
  'notes.exe': { description: null, product: 'Notes', company: 'Notes Project', signature: 'unsigned' },
  'tracker.exe': { description: 'Flight tracker', signature: 'unknown', problem: 'not checked: larger than 1024 MiB' },
};
const aboutChecked = new Set();
let aboutWaiting = null;

const clone = (value) => JSON.parse(JSON.stringify(value));
const find = (name) => state.profiles.find((p) => p.name.toLowerCase() === String(name).trim().toLowerCase());

function fail(error) {
  state.status = error;
  return { ok: false, error };
}

function handle(message) {
  const m = message;
  const profile = m.profile !== undefined ? find(m.profile) : null;
  switch (m.type) {
    case 'get_state': return { ok: true };
    case 'create_profile': {
      const name = String(m.name || '').trim();
      if (!name) return fail('KS205: a profile needs a name');
      if (find(name) || name.toLowerCase() === 'neutral') return fail(`KS205: profile name "${name}" is used already`);
      state.profiles.push({ name, hotkey: null, active: false, triggers: [], priority: 0, entries: [] });
      state.status = `Profile "${name}" created; saved.`;
      return { ok: true };
    }
    case 'copy_profile': {
      const from = find(m.from);
      if (!from) return fail(`no profile named "${m.from}"`);
      if (find(m.to)) return fail(`KS205: profile name "${m.to}" is used already`);
      state.profiles.push({ ...clone(from), name: m.to.trim(), hotkey: null, active: false, triggers: [], priority: 0 });
      state.status = `Profile "${from.name}" copied to "${m.to.trim()}"; saved.`;
      return { ok: true };
    }
    case 'rename_profile': {
      const from = find(m.from);
      if (!from) return fail(`no profile named "${m.from}"`);
      if (find(m.to)) return fail(`KS205: profile name "${m.to}" is used already`);
      if (state.active === from.name) state.active = m.to.trim();
      from.name = m.to.trim();
      state.status = `Profile renamed to "${from.name}"; saved.`;
      return { ok: true };
    }
    case 'delete_profile': {
      const index = state.profiles.findIndex((p) => p === find(m.name));
      if (index < 0) return fail(`no profile named "${m.name}"`);
      state.profiles.splice(index, 1);
      if (state.active.toLowerCase() === String(m.name).toLowerCase()) state.active = 'neutral';
      state.status = `Profile "${m.name}" deleted; saved.`;
      return { ok: true };
    }
    case 'set_hotkey': {
      if (!profile) return fail('no such profile');
      const hotkey = (m.hotkey || '').trim();
      if (hotkey && !/^((Ctrl|Alt|Shift|Win)\+)*([A-Z0-9]|F([1-9]|1[0-9]|2[0-4]))$/i.test(hotkey)) {
        return fail(`KS205: unknown key in hotkey "${hotkey}"`);
      }
      profile.hotkey = hotkey || null;
      state.status = hotkey ? `Hotkey of "${profile.name}" set to ${hotkey}; saved.` : `Hotkey of "${profile.name}" removed; saved.`;
      return { ok: true };
    }
    case 'set_triggers': {
      if (!profile) return fail('no such profile');
      const triggers = (m.triggers || []).map((t) => String(t).trim()).filter(Boolean);
      const own = triggers.find((t) => profile.entries.some((e) =>
        (e.action === 'stop' || e.action === 'suspend') && e.exe.toLowerCase() === t.split('\\').pop().toLowerCase()));
      if (own) return fail(`KS205: trigger "${own}" is a program that this profile stops or freezes`);
      if (!Number.isInteger(m.priority ?? 0)) return fail('malformed request: priority');
      profile.triggers = triggers;
      profile.priority = m.priority ?? 0;
      state.automation.triggers = state.profiles.some((p) => p.triggers.length > 0);
      state.status = triggers.length ? `Triggers of "${profile.name}" set; saved.` : `Triggers of "${profile.name}" removed; saved.`;
      return { ok: true };
    }
    case 'check_program': {
      const protection = protectionOf(m.exe);
      const messages = {
        hard: `${m.exe} is a protected system program and is never touched`,
        warn: `${m.exe} is a remote-access tool. Freezing or closing it can cut you off or break things. Add it anyway?`,
      };
      return { ok: true, data: { protection, message: messages[protection] } };
    }
    case 'add_entry': {
      if (!profile) return fail('no such profile');
      const protection = protectionOf(m.exe);
      if (protection === 'hard') return fail(`${m.exe} is a protected system program and is never touched`);
      if (protection === 'warn' && !m.confirmed) return fail(`${m.exe} is on the warn list and needs confirmation`);
      profile.entries.push(entry(m.exe, state.default_action, { path: m.path || null, confirmed: protection === 'warn' }));
      state.status = `${m.exe} added to "${profile.name}"; saved.`;
      return { ok: true };
    }
    case 'update_entry': {
      if (!profile || !profile.entries[m.index]) return fail('no such entry');
      const e = m.entry;
      if (e.action === 'start' && !e.path) return fail('KS205: an entry with action = "start" needs a path');
      profile.entries[m.index] = entry(e.exe, e.action, {
        path: e.path || null, args: e.args || [], cwd: e.cwd || null,
        on_timeout: e.on_timeout || 'kill', restart_on_neutral: !!e.restart_on_neutral,
        trim: e.action === 'suspend' ? (e.trim ?? true) : null, confirmed: !!e.confirmed,
      });
      state.status = `${e.exe} in "${profile.name}" changed; saved.`;
      return { ok: true };
    }
    case 'remove_entry': {
      if (!profile) return fail('no such profile');
      profile.entries.splice(m.index, 1);
      state.status = `Entry removed from "${profile.name}"; saved.`;
      return { ok: true };
    }
    case 'pick_executable':
      return { ok: true, data: { exe: 'editor.exe', path: 'C:\\Program Files\\Editor\\editor.exe', protection: 'none' } };
    case 'drop_files': {
      // M2: the browser gives no paths; the mock makes some up.
      const names = m.mock_names || [];
      if (names.length === 0) return fail('no file was dropped (drop files from Explorer)');
      const files = [];
      const refused = [];
      for (const name of names) {
        if (/\.exe$/i.test(name)) files.push({ exe: name, path: `C:\\Dropped\\${name}`, protection: protectionOf(name) });
        else refused.push(`${name} is not a program (.exe)`);
      }
      return { ok: true, data: { files, refused } };
    }
    case 'drop_done': {
      const notAdded = m.not_added || [];
      const total = m.added + notAdded.length;
      state.status = notAdded.length
        ? `${m.added} of ${total} added to "${m.profile}". Not added: ${notAdded.join('; ')}`
        : `${m.added} program${m.added === 1 ? '' : 's'} added to "${m.profile}"; saved.`;
      return { ok: true };
    }
    case 'running': return { ok: true, data: clone(running) };
    case 'icons': {
      if (!Array.isArray(m.programs) || m.programs.length > 256) return fail('at most 256 icons per request');
      if (!(m.size >= 16 && m.size <= 96)) return fail(`icon size ${m.size} is outside 16 to 96`);
      // A coloured square per program; programs without a path that are
      // not running have none, as on the real host.
      return { ok: true, data: m.programs.map((p) => (p.path || running.some((r) => r.exe === p.exe)) ? mockIcon(p.exe) : null) };
    }
    case 'program_info': {
      if (!Array.isArray(m.programs) || m.programs.length > 256) return fail('at most 256 programs per request');
      // As the host: nothing for programs without a file; the signature
      // arrives a moment later, then the `program_info` event.
      const data = m.programs.map((p) => {
        if (!p.path && !running.some((r) => r.exe === p.exe) && !mockAbout[p.exe.toLowerCase()]) return null;
        const about = { product: null, company: null, version: '1.0.0.0', catalog: false, problem: null, publisher: null, ...(mockAbout[p.exe.toLowerCase()] || { description: null, signature: 'unsigned' }) };
        if (aboutChecked.has(p.exe.toLowerCase())) return { ...about, pending: false };
        if (!aboutWaiting) {
          aboutWaiting = setTimeout(() => {
            aboutWaiting = null;
            for (const q of m.programs) aboutChecked.add(q.exe.toLowerCase());
            emit({ v: 1, event: 'program_info' });
          }, 700);
        }
        return { ...about, signature: 'pending', publisher: null, problem: null, pending: true };
      });
      return { ok: true, data };
    }
    case 'unassigned': return { ok: true, data: clone(unassigned[profile ? profile.name : ''] || []) };
    case 'dismiss': {
      const list = unassigned[profile ? profile.name : ''] || [];
      const index = list.findIndex((u) => u.exe === m.exe);
      if (index >= 0) list.splice(index, 1);
      state.status = `${m.exe} dismissed for "${m.profile}".`;
      return { ok: true };
    }
    case 'apply': {
      if (state.applying) return fail('a switch is still running');
      state.applying = true;
      state.status = `Applying "${m.profile}"…`;
      setTimeout(() => {
        state.applying = false;
        state.active = m.profile;
        state.status = `Applied "${m.profile}".`;
        // M5: a clean switch clears the markers.
        for (const p of state.profiles) for (const e of p.entries) e.errors = [];
        emit({ v: 1, event: 'applied', state: view() });
      }, 600);
      return { ok: true };
    }
    case 'set_update_mode': {
      if (!['auto', 'manual', 'off'].includes(m.mode)) return fail(`unknown update mode "${m.mode}"`);
      state.update.mode = m.mode;
      state.status = `Update mode ${m.mode}.`;
      if (m.mode !== 'off' && !state.update.checking) check();
      return { ok: true };
    }
    case 'check_updates':
      if (state.update.checking) return fail('a check is still running');
      state.status = 'Checking for updates…';
      check();
      return { ok: true };
    case 'install_update':
      if (!state.update.available) return fail('no newer version is known; check first');
      state.status = `Installing ${state.update.available} in a separate window…`;
      return { ok: true };
    case 'set_automation':
      if (typeof m.on !== 'boolean') return fail('malformed request: on');
      state.automation.on = m.on;
      state.status = `Automatic switching ${m.on ? 'on' : 'paused'}.`;
      return { ok: true };
    case 'set_autostart':
      state.autostart = !!m.on;
      state.status = `Start at sign-in ${m.on ? 'on' : 'off'}.`;
      return { ok: true };
    case 'ready':
    case 'log':
    case 'drive_done':
      return null;
    default:
      return fail(`malformed request: unknown type "${m.type}"`);
  }
}

/** A check that finds 1.1.0 after a moment. */
function check() {
  state.update.checking = true;
  setTimeout(() => {
    state.update.checking = false;
    state.update.available = '1.1.0';
    state.update.last_check = Math.floor(Date.now() / 1000);
    state.status = 'Version 1.1.0 is available.';
    emit({ v: 1, event: 'update_checked', state: view() });
  }, 800);
}

function view() {
  const copy = clone(state);
  for (const p of copy.profiles) p.active = p.name.toLowerCase() === state.active.toLowerCase();
  return copy;
}

let emit = () => {};

/** Returns the send function; replies go to `deliver`. */
export function connect(deliver) {
  emit = deliver;
  return (message) => {
    const reply = handle(message);
    if (reply === null) return;
    setTimeout(() => deliver({ v: 1, id: message.id, ...reply, state: view() }), 30);
  };
}

/** A square in a colour derived from the name, as an SVG `data:` URL. */
function mockIcon(exe) {
  let hue = 0;
  for (const c of exe) hue = (hue * 31 + c.charCodeAt(0)) % 360;
  const svg = `<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 16 16"><rect x="1" y="1" width="14" height="14" rx="3" fill="hsl(${hue} 60% 50%)"/></svg>`;
  return `data:image/svg+xml,${encodeURIComponent(svg)}`;
}
