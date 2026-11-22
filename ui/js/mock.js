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
    confirmed: false, protection: protectionOf(exe), ...extra,
  };
}

const state = {
  config_file: 'C:\\Users\\you\\AppData\\Roaming\\kaltstellen\\config.toml',
  active: 'Flight',
  default_action: 'suspend',
  default_trim: true,
  autostart: false,
  applying: false,
  status: 'Editing the sample configuration (mock backend, nothing is saved).',
  profiles: [
    {
      name: 'Flight', hotkey: 'Ctrl+Shift+Alt+F1', active: true,
      entries: [
        entry('browser.exe', 'suspend'),
        entry('mail.exe', 'suspend', { trim: false }),
        entry('chat.exe', 'stop', { restart_on_neutral: true, args: ['--minimized'] }),
        entry('tracker.exe', 'start', { path: 'C:\\Tools\\Tracker\\tracker.exe', args: ['--profile', 'flight'] }),
        entry('anydesk.exe', 'ignore'),
      ],
    },
    {
      name: 'Music production', hotkey: 'Ctrl+Shift+Alt+F2', active: false,
      entries: [
        entry('browser.exe', 'suspend'),
        entry('sync.exe', 'stop', { on_timeout: 'keep' }),
      ],
    },
    { name: 'Quiet evening', hotkey: null, active: false, entries: [] },
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
      state.profiles.push({ name, hotkey: null, active: false, entries: [] });
      state.status = `Profile "${name}" created; saved.`;
      return { ok: true };
    }
    case 'copy_profile': {
      const from = find(m.from);
      if (!from) return fail(`no profile named "${m.from}"`);
      if (find(m.to)) return fail(`KS205: profile name "${m.to}" is used already`);
      state.profiles.push({ ...clone(from), name: m.to.trim(), hotkey: null, active: false });
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
    case 'running': return { ok: true, data: clone(running) };
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
        emit({ v: 1, event: 'applied', state: view() });
      }, 600);
      return { ok: true };
    }
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
