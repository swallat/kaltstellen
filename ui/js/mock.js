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
  stop_timeout_s: 8,
  minimize_before_suspend: true,
  locations: {
    settings_file: 'C:\\Users\\you\\AppData\\Roaming\\kaltstellen\\settings.json',
    data_dir: 'C:\\Users\\you\\AppData\\Local\\kaltstellen',
    log_file: 'C:\\Users\\you\\AppData\\Local\\kaltstellen\\kaltstellen.log',
    last_profile: 'Flight',
  },
  autostart: false,
  automation: { on: true, triggers: true, note: null },
  learn: true,
  notify_new: true,
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

// Review states: ?state=update | locked | notriggers.
const reviewState = new URLSearchParams(location.search).get('state');
if (reviewState === 'update') Object.assign(state.update, { available: '1.1.0' });
if (reviewState === 'locked') state.update.mode_locked = true;
if (reviewState === 'notriggers') {
  state.automation.triggers = false;
  for (const p of state.profiles) p.triggers = [];
}

const running = [
  { exe: 'browser.exe', private_bytes: 1_340_000_000 },
  { exe: 'game-launcher.exe', private_bytes: 412_000_000 },
  { exe: 'mail.exe', private_bytes: 380_000_000 },
  { exe: 'explorer.exe', private_bytes: 160_000_000 },
  { exe: 'anydesk.exe', private_bytes: 48_000_000 },
  { exe: 'notes.exe', private_bytes: 35_500_000 },
].map((p) => ({ ...p, protection: protectionOf(p.exe) }));

const neverSuggested = new Set();
const unassigned = {
  'Flight': [
    { exe: 'game-launcher.exe', path: 'C:\\Games\\Launcher\\game-launcher.exe', new: true, started: true },
    { exe: 'notes.exe', path: null, new: false, tray: true },
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
    case 'set_watch': {
      if (!profile) return fail('no such profile');
      profile.watch = !!m.on;
      state.status = m.on ? `"${profile.name}" now handles its programs that start later; saved.` : `"${profile.name}" no longer handles programs that start later; saved.`;
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
    case 'snapshot': {
      const mockKind = (exe) => ({
        'browser.exe': { id: 'browser', plural: 'browsers', label: 'a browser', known: true, source: 'name' },
        'mail.exe': { id: 'mail', plural: 'mail programs', label: 'a mail program', known: true, source: 'name' },
        'sync.exe': { id: 'cloud_sync', plural: 'cloud sync programs', label: 'a cloud sync program', known: true, source: 'name' },
        'chat.exe': { id: 'chat', plural: 'chat programs', label: 'a chat program', known: true, source: 'name' },
      }[exe] || { id: 'unknown', plural: 'programs of unknown kind', label: 'a program', known: false, source: 'none' });
      const profile2 = m.mode === 'update' ? find(m.profile) : null;
      if (m.mode === 'update' && !profile2) return fail(`no profile named "${m.profile}"`);
      const has = (exe) => profile2 && profile2.entries.some((e) => e.exe.toLowerCase() === exe.toLowerCase());
      const row = (exe, extra = {}) => ({
        exe, path: `C:\\Apps\\${exe}`, private_bytes: 0, protection: protectionOf(exe),
        elevated: false, background: false, held: null, first_seen: null,
        kind: mockKind(exe),
        suggestion: exe === 'browser.exe' || exe === 'mail.exe' ? { action: 'suspend', reason: 'state and sign-in stay, nothing is lost; notifications wait' } : null,
        windows: { count: exe === 'browser.exe' ? 3 : 1, dialog: exe === 'mail.exe', unsaved: exe === 'browser.exe', hung: false },
        ...extra,
      });
      const rows = running.filter((r) => r.protection !== 'hard' && r.exe !== 'chat.exe')
        .map((r) => row(r.exe, { private_bytes: r.private_bytes, background: r.exe === 'notes.exe', tray: r.exe === 'mail.exe' || r.exe === 'sync.exe' }));
      const existing = rows.filter((r) => has(r.exe)).length;
      return { ok: true, data: {
        running: rows.filter((r) => !has(r.exe)),
        held: [row('chat.exe', { held: 'suspend' })].filter((r) => !has(r.exe)),
        seen: [row('old-tool.exe', { first_seen: 1700000000 }), row('anydesk-old.exe', { first_seen: 1690000000 })],
        existing,
      } };
    }
    case 'create_profile_from': {
      const name = String(m.name || '').trim();
      if (!name) return fail('KS205: a profile needs a name');
      if (find(name) || name.toLowerCase() === 'neutral') return fail(`KS205: profile name "${name}" is used already`);
      for (const e of m.entries) {
        const prot = protectionOf(e.exe);
        if (prot === 'hard') return fail(`${e.exe} is a protected system program and is never touched`);
        if (prot === 'warn' && !e.confirmed && (e.action === 'stop' || e.action === 'suspend')) return fail(`${e.exe} is on the warn list and needs confirmation`);
        if (e.action === 'start' && !e.path) return fail('KS205: an entry with action = "start" needs a path');
      }
      state.profiles.push({ name, hotkey: null, active: false, triggers: [], priority: 0,
        entries: m.entries.map((e) => entry(e.exe, e.action, { path: e.path || null, confirmed: !!e.confirmed })) });
      state.status = `Profile "${name}" created with ${m.entries.length} entries; saved.`;
      return { ok: true };
    }
    case 'add_entries': {
      if (!profile) return fail('no such profile');
      for (const e of m.entries) {
        if (protectionOf(e.exe) === 'hard') return fail(`${e.exe} is a protected system program and is never touched`);
        if (e.action === 'start' && !e.path) return fail('KS205: an entry with action = "start" needs a path');
      }
      for (const e of m.entries) profile.entries.push(entry(e.exe, e.action, { path: e.path || null, confirmed: !!e.confirmed }));
      state.status = `${m.entries.length} entries added to "${profile.name}"; saved.`;
      return { ok: true };
    }
    case 'measure_start': return { ok: true };
    case 'measure': {
      const data = m.programs.map((exe, i) => ({
        exe, frozen: exe === 'chat.exe', private_bytes: 0,
        cpu: exe === 'chat.exe' ? null : [0.12, 0.004, 0.03, 0.0][i % 4],
        io_per_s: exe === 'chat.exe' ? null : [3_500_000, 2048, 90_000, 0][i % 4],
        net_down_per_s: exe === 'chat.exe' ? null : [2_400_000, 0, 51_200, null][i % 4],
        net_up_per_s: exe === 'chat.exe' ? null : [120_000, 0, 2048, null][i % 4],
        tcp: i % 3, udp: i % 2,
        heavy: i === 0 ? '12 % CPU, 3.3 MB/s I/O' : (exe === 'browser.exe' ? '1.2 GB memory' : null),
      }));
      return { ok: true, data: { interval_ms: 1500, programs: data } };
    }
    case 'preview': {
      const p = state.profiles.find((q) => q.name.toLowerCase() === String(m.profile).toLowerCase());
      if (!p) return fail(`no profile named "${m.profile}"`);
      const step = { suspend: 'suspend', stop: 'stop', start: 'start' };
      const items = p.entries.filter((e) => e.action !== 'ignore').map((e) => {
        const on = running.some((r) => r.exe.toLowerCase() === e.exe.toLowerCase());
        const verb = e.action === 'stop' ? 'stop' : e.action === 'start' ? 'start' : 'freeze';
        return {
          step: step[e.action] || e.action, program: e.exe,
          outcome: on || e.action === 'start' ? 'will' : 'no_change',
          text: on || e.action === 'start' ? `would ${verb}${on ? ' 1 process' : ''}` : 'not running',
          processes: on ? 1 : 0, private_bytes: on ? 50 * 1048576 : 0,
        };
      });
      const sum = items.filter((i) => i.outcome === 'will' && i.step !== 'start').reduce((n, i) => n + i.private_bytes, 0);
      return { ok: true, data: { profile: p.name, items, problems: [], private_bytes: sum } };
    }
    case 'templates':
      return { ok: true, data: [
        { id: 'focus', name: 'Focus', purpose: 'Quiet for work: nothing chimes, nothing updates, the browser stays.', rules: [
          { kind: 'chat', label: 'chat programs', action: 'suspend' }, { kind: 'mail', label: 'mail programs', action: 'suspend' },
          { kind: 'updater', label: 'updaters and helpers', action: 'stop' }, { kind: 'browser', label: 'browsers', action: 'ignore' }] },
        { id: 'gaming', name: 'Gaming', purpose: 'Memory and processor for the game: what runs beside it waits.', rules: [
          { kind: 'browser', label: 'browsers', action: 'suspend' }, { kind: 'mail', label: 'mail programs', action: 'suspend' },
          { kind: 'launcher', label: 'launchers and store clients', action: 'ignore' }] },
      ] };
    case 'compare': {
      const a = state.profiles.find((p) => p.name.toLowerCase() === String(m.a).toLowerCase());
      const b = state.profiles.find((p) => p.name.toLowerCase() === String(m.b).toLowerCase());
      if (!a) return fail(`no profile named "${m.a}"`);
      if (!b) return fail(`no profile named "${m.b}"`);
      const same = (x, y) => x.exe.toLowerCase() === y.exe.toLowerCase() && (!x.path || !y.path || x.path.toLowerCase() === y.path.toLowerCase());
      const text = (e) => {
        const o = [];
        if (e.action === 'suspend') o.push(e.trim ? 'trim' : 'no trim');
        if (e.action === 'stop') o.push(e.on_timeout === 'keep' ? 'keep it if it does not close' : 'end it if it does not close');
        if (e.restart_on_neutral) o.push('restart on neutral');
        if (e.confirmed) o.push('confirmed');
        return [e.action, ...o].join(' · ');
      };
      const taken = new Set();
      const rows = [];
      for (const e of a.entries) {
        const i = b.entries.findIndex((o, k) => !taken.has(k) && same(e, o));
        if (i < 0) { rows.push({ exe: e.exe, path: e.path, a: text(e), b: null, difference: 'only_a' }); continue; }
        taken.add(i);
        const o = b.entries[i];
        const difference = o.action !== e.action ? 'action' : text(o) !== text(e) ? 'options' : null;
        rows.push({ exe: e.exe, path: e.path || o.path, a: text(e), b: text(o), difference });
      }
      b.entries.forEach((o, i) => { if (!taken.has(i)) rows.push({ exe: o.exe, path: o.path, a: null, b: text(o), difference: 'only_b' }); });
      const pair = (x, y) => (x !== y ? { a: x, b: y } : null);
      return { ok: true, data: { rows, meta: { hotkey: pair(a.hotkey || '', b.hotkey || ''), triggers: pair((a.triggers || []).join(', '), (b.triggers || []).join(', ')), priority: pair(a.priority || 0, b.priority || 0) } } };
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
    case 'set_defaults': {
      if (!['suspend', 'stop', 'ignore'].includes(m.default_action)) return fail(`unknown default action "${m.default_action}"`);
      if (!Number.isInteger(m.stop_timeout_s) || m.stop_timeout_s < 1 || m.stop_timeout_s > 300) return fail('the time to close a program is 1 to 300 seconds');
      state.default_action = m.default_action;
      state.default_trim = !!m.trim;
      state.stop_timeout_s = m.stop_timeout_s;
      state.minimize_before_suspend = !!m.minimize_before_suspend;
      state.status = 'Program defaults changed (they apply from the next switch); saved.';
      return { ok: true };
    }
    case 'open_location':
      if (!['config_folder', 'log', 'log_folder'].includes(m.target)) return fail(`unknown location "${m.target}"`);
      state.status = `Mock: would open ${m.target}.`;
      return { ok: true };
    case 'reload_config':
      state.status = 'Configuration read again.';
      return { ok: true };
    case 'suggestions': {
      const entries = profile ? profile.entries : [];
      const known = entries.findIndex((e) => e.exe === 'chat.exe');
      const list = [];
      if (!state.learn || neverSuggested.has('chat.exe')) return { ok: true, data: list };
      list.push({ exe: 'chat.exe', path: null, action: 'stop', reason: 'In 4 of the last 5 activations chat.exe was gone when you switched back (it ended or you closed it).', seen: 4, of: 5, entry: known >= 0 ? known : null, tray: true });
      return { ok: true, data: list };
    }
    case 'suggestion_done':
      if (m.never) neverSuggested.add(m.exe);
      return { ok: true };
    case 'set_notify_new':
      state.notify_new = !!m.on;
      state.status = state.notify_new ? 'New programs are announced while a profile is active.' : 'New programs are no longer announced.';
      return { ok: true };
    case 'set_learn':
      state.learn = !!m.on;
      state.status = state.learn ? 'Suggestions from use are on.' : 'Suggestions from use are off; nothing is recorded.';
      return { ok: true };
    case 'forget_learned':
      neverSuggested.clear();
      state.status = 'What was learned from use is deleted.';
      return { ok: true };
    case 'forget_last_profile':
      state.locations.last_profile = null;
      state.status = 'The offer to apply the last profile after sign-in is cleared.';
      return { ok: true };
    case 'check_updates':
      if (state.update.checking) return fail('a check is still running');
      state.status = 'Checking for updates…';
      check();
      return { ok: true };
    case 'install_update':
      if (!state.update.available) return fail('no newer version is known; check first');
      state.status = `Installing ${state.update.available} in a separate window…`;
      return { ok: true };
    case 'helper_status':
      setTimeout(() => emit({ v: 1, event: 'helper_status', data: { state: 'not_installed', answers: false, version: null, roles: [], notes: [] } }), 50);
      return { ok: true };
    case 'helper_install':
      state.status = 'Installing the background helper in a separate window…';
      return { ok: true };
    case 'helper_uninstall':
      state.status = 'Removing the background helper in a separate window…';
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
