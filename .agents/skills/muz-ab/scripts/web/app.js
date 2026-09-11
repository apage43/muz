import { AudioTransport } from './audio.js';

const $ = (id) => document.getElementById(id);
const slots = [...document.querySelectorAll('[data-slot]')];
const responses = [...document.querySelectorAll('[data-response]')];
let config;
let session;
let transport = new AudioTransport();
let ready = false;
let busy = false;
let waveform;
let trialOpened = 0;
let drag;
let lastDrawing = '';
let history = [];

function status(message, error = false) {
  $('status').textContent = message;
  $('status').classList.toggle('error', error);
}

async function api(path, body) {
  const response = await fetch(path, body === undefined ? {} : {
    method: 'POST', headers: { 'Content-Type': 'application/json' }, body: JSON.stringify(body),
  });
  const data = await response.json();
  if (!response.ok) {
    const error = new Error(typeof data.error === 'string' ? data.error : `Request failed (${response.status})`);
    error.status = response.status;
    throw error;
  }
  return data;
}

function key(suffix) { return `muz-ab:${config.pair_id}:${suffix}`; }
function saved(suffix, fallback) {
  try { return JSON.parse(localStorage.getItem(key(suffix))) ?? fallback; }
  catch { return fallback; }
}
function save(suffix, value) {
  try { localStorage.setItem(key(suffix), JSON.stringify(value)); }
  catch { status(`Browser storage is unavailable. Your session is saved on the server: ${session?.id ?? ''}`, true); }
}

function remember() {
  save('session', session.id);
  const item = { id: session.id, mode: session.mode, status: session.status,
    trials: session.planned_trials, created: session.created_at };
  history = [item, ...history.filter((entry) => entry.id !== item.id)];
  save('history', history);
  renderHistory();
}

function renderHistory() {
  $('history').replaceChildren();
  $('history-panel').hidden = history.length === 0;
  for (const entry of history) {
    const row = document.createElement('li');
    const button = document.createElement('button');
    const date = new Date(entry.created);
    button.textContent = `${entry.mode.toUpperCase()} · ${entry.trials} planned · ${entry.status} · ${Number.isNaN(date.getTime()) ? entry.created : date.toLocaleString()}`;
    button.onclick = () => guarded(async () => {
      transport.pause();
      session = await api(`/api/sessions/${encodeURIComponent(entry.id)}`);
      remember();
      await showSession();
    });
    row.append(button);
    $('history').append(row);
  }
}

async function guarded(action) {
  if (busy) return;
  busy = true;
  updateControls();
  try { await action(); }
  catch (error) { status(error.message, true); }
  finally { busy = false; updateControls(); }
}

function region() {
  const state = transport.snapshot();
  return { start: state.start, end: state.end, loop: state.loop, volume: state.volume };
}

function applyRegion(start, end, persist = true) {
  const duration = transport.snapshot().duration;
  const minimum = Math.min(0.02, duration);
  if (!Number.isFinite(start) || !Number.isFinite(end) || end - start < minimum - 1e-6 || start < 0 || end > duration + 1e-6) {
    throw new Error(`Select at least ${(minimum * 1000).toFixed(0)} ms inside the clip.`);
  }
  transport.setRegion(start, Math.min(end, duration));
  syncRegionFields();
  if (persist) save('region', region());
  lastDrawing = '';
}

function syncRegionFields() {
  const state = transport.snapshot();
  $('loop-start').value = state.start.toFixed(6);
  $('loop-end').value = state.end.toFixed(6);
  $('loop-start').max = String(state.duration);
  $('loop-end').max = String(state.duration);
  $('loop-enabled').checked = state.loop;
  $('volume').value = state.volume;
  $('volume-value').textContent = `${Math.round(state.volume * 100)}%`;
}

async function showSession() {
  $('setup').hidden = true;
  $('session').hidden = session.status !== 'active';
  $('results').hidden = session.status === 'active';
  if (session.status !== 'active') {
    ready = false;
    transport.pause();
    showResults();
    return;
  }
  const previous = ready ? region() : saved('region', null);
  ready = false;
  waveform = null;
  lastDrawing = '';
  $('mode-badge').textContent = session.mode === 'abx' ? 'BLIND ABX' : 'BLIND PREFERENCE';
  $('progress').textContent = `Trial ${session.trial.number} / ${session.planned_trials}`;
  $('instructions').textContent = session.mode === 'abx'
    ? 'A and B are references. X is one of them. Switch and loop as much as you need, then identify X. A/B source identities reshuffle each trial; no answers are revealed until the end.'
    : 'Switch and loop as much as you need, then choose A, B, or no preference. A/B source identities reshuffle each trial; running vote totals stay hidden until the end.';
  $('response-title').textContent = session.mode === 'abx' ? 'Which reference is X?' : 'Which version do you prefer?';
  for (const button of slots) button.hidden = !(button.dataset.slot in session.trial.audio);
  for (const button of responses) {
    button.hidden = button.dataset.response === 'tie' && session.mode === 'abx';
    button.textContent = button.dataset.response === 'tie' ? 'No preference'
      : session.mode === 'abx' ? `X is ${button.dataset.response}` : `Prefer ${button.dataset.response}`;
  }
  $('note').value = '';
  status(`Loading aligned audio · ${session.completed_trials} of ${session.planned_trials} responses recorded.`);
  await transport.load(session.trial.audio, { sampleRate: config.sample_rate });
  ready = true;
  const duration = transport.snapshot().duration;
  if (previous && previous.start >= 0 && previous.end <= duration + 1e-6 && previous.end > previous.start) {
    applyRegion(previous.start, Math.min(previous.end, duration), false);
    transport.setLoop(previous.loop);
    transport.setVolume(previous.volume);
  }
  syncRegionFields();
  waveform = transport.waveform(1024);
  trialOpened = performance.now();
  status(`${session.completed_trials} of ${session.planned_trials} responses recorded. Trial ${session.trial.number} is ready; press Play.`);
}

function showResults() {
  const result = session.result;
  const complete = result.completed_as_planned;
  $('result-title').textContent = complete ? 'Session complete — identities revealed' : 'Ended early — exploratory results';
  $('result-mode').textContent = result.mode.toUpperCase();
  $('identities').replaceChildren();
  for (const [index, label] of result.labels.entries()) {
    const element = document.createElement('div');
    element.className = 'identity';
    const caption = document.createElement('small');
    caption.textContent = `Candidate ${index + 1} (A/B positions were randomized)`;
    const name = document.createElement('strong');
    name.textContent = label;
    element.append(caption, name);
    $('identities').append(element);
  }
  $('score-label').textContent = result.mode === 'abx' ? 'Correct identifications' : 'Candidate 1 / Candidate 2 / Ties';
  $('score').textContent = result.mode === 'abx' ? `${result.correct} / ${result.trials}`
    : `${result.counts.candidate_0} / ${result.counts.candidate_1} / ${result.counts.tie}`;
  $('p-value').textContent = result.p_value === null ? 'Not reported' : result.p_value < 0.0001
    ? result.p_value.toExponential(3) : result.p_value.toFixed(4);
  $('interval').textContent = result.confidence_interval === null ? 'Not reported'
    : result.confidence_interval.map((value) => `${(value * 100).toFixed(1)}%`).join('–');
  $('interpretation').textContent = !complete
    ? 'The planned trial count was not reached. These counts are descriptive; no fixed-length statistical claim is reported.'
    : result.mode === 'preference'
      ? `${result.decisive_trials} decisive choices; ${result.counts.tie} ties. The interval is Candidate 1’s share of decisive choices. Preference does not establish that you can identify the versions reliably.`
      : result.p_value <= 0.05
        ? 'Above chance in this fixed-length session. Confirm independently before drawing broader conclusions; this does not establish which version is better.'
        : 'This session did not establish reliable discrimination. That does not prove the versions sound identical.';
  $('test-description').textContent = `${result.trials} / ${result.planned_trials} planned trials. ${result.test}`;
  $('caveats').replaceChildren();
  for (const caveat of result.caveats) {
    const item = document.createElement('li');
    item.textContent = caveat;
    $('caveats').append(item);
  }
  $('export-json').href = `/api/sessions/${encodeURIComponent(session.id)}/export.json`;
  $('export-csv').href = `/api/sessions/${encodeURIComponent(session.id)}/export.csv`;
  $('trial-audit').textContent = JSON.stringify(result.trial_records, null, 2);
  status('Results and trial records are saved on the server. Download JSON or CSV to keep a portable copy.');
}

function canAnswer(state) {
  return ready && !busy && session?.status === 'active'
    && Object.keys(session.trial.audio).every((slot) => (state.listened_seconds[slot] ?? 0) >= 0.1);
}

function updateControls() {
  const state = transport.snapshot();
  $('start-session').disabled = busy;
  $('finish-early').disabled = busy;
  for (const element of [...slots, $('play'), $('restart'), $('full-clip'), $('loop-start'), $('loop-end'), $('loop-enabled'), $('volume')]) {
    element.disabled = busy || !ready;
  }
  for (const button of slots) button.setAttribute('aria-pressed', String(state.selected === button.dataset.slot));
  for (const button of responses) button.disabled = !canAnswer(state);
  $('play').textContent = state.playing ? 'Pause' : 'Play';
  $('audio-state').textContent = `${state.playing ? 'Playing' : 'Paused'} · ${state.selected || 'A'}`;
  if (ready && session?.status === 'active') {
    const remaining = Object.keys(session.trial.audio).filter((slot) => (state.listened_seconds[slot] ?? 0) < 0.1);
    $('listen-hint').textContent = remaining.length ? `Listen to ${remaining.join(', ')} before recording a response.`
      : 'Ready to record. Keep comparing if you need more time.';
  }
  $('clock').textContent = `${state.position.toFixed(3)} / ${state.duration.toFixed(3)} s`;
  return state;
}

function draw(state) {
  const canvas = $('waveform');
  const box = canvas.getBoundingClientRect();
  if (!box.width) return;
  const scale = window.devicePixelRatio || 1;
  const width = Math.round(box.width * scale), height = Math.round(box.height * scale);
  if (canvas.width !== width || canvas.height !== height) { canvas.width = width; canvas.height = height; }
  const selection = drag?.preview ?? state;
  const drawKey = `${width}:${height}:${Math.round(state.position / (state.duration || 1) * width)}:${selection.start}:${selection.end}:${ready}`;
  if (drawKey === lastDrawing) return;
  lastDrawing = drawKey;
  const context = canvas.getContext('2d');
  context.setTransform(scale, 0, 0, scale, 0, 0);
  const w = box.width, h = box.height;
  context.clearRect(0, 0, w, h);
  if (!waveform || !state.duration) {
    context.fillStyle = '#acbdca'; context.font = '14px system-ui';
    context.fillText('Loading aligned waveform…', 18, h / 2);
    return;
  }
  const x = (seconds) => seconds / state.duration * w;
  const start = x(selection.start), end = x(selection.end);
  context.fillStyle = '#7cd9c422'; context.fillRect(start, 0, end - start, h);
  const maximum = Math.max(0.001, ...waveform);
  context.strokeStyle = '#90b2c5'; context.lineWidth = Math.max(1, w / waveform.length * 0.8);
  context.beginPath();
  for (let index = 0; index < waveform.length; index++) {
    const position = (index + 0.5) / waveform.length * w;
    const amplitude = waveform[index] / maximum * (h - 38) * 0.46;
    context.moveTo(position, (h - 20) / 2 - amplitude);
    context.lineTo(position, (h - 20) / 2 + amplitude);
  }
  context.stroke();
  context.fillStyle = '#10192399'; context.fillRect(0, 0, start, h); context.fillRect(end, 0, w - end, h);
  context.strokeStyle = '#7cd9c4'; context.lineWidth = 2;
  context.beginPath(); context.moveTo(start, 0); context.lineTo(start, h - 20); context.moveTo(end, 0); context.lineTo(end, h - 20); context.stroke();
  context.strokeStyle = '#ffffff'; context.lineWidth = 1.5;
  context.beginPath(); context.moveTo(x(state.position), 0); context.lineTo(x(state.position), h - 20); context.stroke();
  context.font = '11px ui-monospace, monospace'; context.fillStyle = '#acbdca';
  for (let tick = 0; tick <= 4; tick++) {
    context.textAlign = tick === 0 ? 'left' : tick === 4 ? 'right' : 'center';
    context.fillText(`${(state.duration * tick / 4).toFixed(2)}s`, w * tick / 4, h - 5);
  }
}

function frame() { draw(updateControls()); requestAnimationFrame(frame); }

function pointerTime(event) {
  const box = $('waveform').getBoundingClientRect();
  return Math.max(0, Math.min(1, (event.clientX - box.left) / box.width)) * transport.snapshot().duration;
}

$('waveform').addEventListener('pointerdown', (event) => {
  if (!ready || busy || (event.button !== 0 && event.pointerType === 'mouse')) return;
  const state = transport.snapshot();
  const time = pointerTime(event);
  const tolerance = 9 / $('waveform').getBoundingClientRect().width * state.duration;
  drag = { anchor: time, x: event.clientX, moved: false, original: region(), preview: region(),
    mode: Math.abs(time - state.start) < tolerance ? 'start' : Math.abs(time - state.end) < tolerance ? 'end' : 'range' };
  $('waveform').setPointerCapture(event.pointerId);
});
$('waveform').addEventListener('pointermove', (event) => {
  if (!drag) return;
  const time = pointerTime(event);
  drag.moved ||= Math.abs(event.clientX - drag.x) > 3;
  if (!drag.moved) return;
  const minimum = Math.min(0.02, transport.snapshot().duration);
  if (drag.mode === 'start') drag.preview.start = Math.min(time, drag.original.end - minimum);
  else if (drag.mode === 'end') drag.preview.end = Math.max(time, drag.original.start + minimum);
  else { drag.preview.start = Math.min(drag.anchor, time); drag.preview.end = Math.max(drag.anchor, time); }
});
$('waveform').addEventListener('pointerup', (event) => {
  if (!drag) return;
  const selection = drag; drag = null;
  try {
    if (selection.moved) applyRegion(selection.preview.start, selection.preview.end);
    else transport.seek(pointerTime(event));
  } catch (error) { status(error.message, true); }
  lastDrawing = '';
});
$('waveform').addEventListener('pointercancel', () => { drag = null; lastDrawing = ''; });

$('setup-form').onsubmit = (event) => {
  event.preventDefault();
  guarded(async () => {
    session = await api('/api/sessions', { mode: new FormData(event.currentTarget).get('mode'), planned_trials: Number($('planned-trials').value) });
    remember();
    await showSession();
  });
};
$('play').onclick = () => guarded(async () => {
  if (!ready) return;
  if (transport.snapshot().playing) transport.pause(); else await transport.play();
});
$('restart').onclick = () => { if (ready && !busy) transport.seek(transport.snapshot().start); };
for (const button of slots) button.onclick = () => { if (ready && !busy) transport.select(button.dataset.slot); };
$('full-clip').onclick = () => { if (ready && !busy) applyRegion(0, transport.snapshot().duration); };
for (const id of ['loop-start', 'loop-end']) $(id).onchange = () => {
  try { applyRegion(Number($('loop-start').value), Number($('loop-end').value)); }
  catch (error) { status(error.message, true); syncRegionFields(); }
};
$('loop-enabled').onchange = () => { transport.setLoop($('loop-enabled').checked); save('region', region()); };
$('volume').oninput = () => { transport.setVolume(Number($('volume').value)); $('volume-value').textContent = `${Math.round(Number($('volume').value) * 100)}%`; save('region', region()); };

for (const button of responses) button.onclick = () => guarded(async () => {
  const state = transport.snapshot();
  if (!ready || !Object.keys(session.trial.audio).every((slot) => (state.listened_seconds[slot] ?? 0) >= 0.1)) return;
  transport.pause();
  try {
    session = await api(`/api/sessions/${encodeURIComponent(session.id)}/answer`, {
      trial_id: session.trial.id, response: button.dataset.response, loop_start: state.start, loop_end: state.end,
      note: $('note').value, listened_seconds: state.listened_seconds, switches: state.switches,
      elapsed_seconds: (performance.now() - trialOpened) / 1000,
    });
  } catch (error) {
    if (error.status !== 409) throw error;
    session = await api(`/api/sessions/${encodeURIComponent(session.id)}`);
  }
  remember();
  await showSession();
});
$('finish-early').onclick = () => {
  if (!confirm('End this session and reveal identities now? An incomplete session reports descriptive counts only, with no fixed-length p-value.')) return;
  guarded(async () => {
    transport.pause();
    session = await api(`/api/sessions/${encodeURIComponent(session.id)}/finish`, {});
    remember();
    await showSession();
  });
};
$('new-session').onclick = () => {
  transport.pause(); ready = false;
  $('session').hidden = true; $('results').hidden = true; $('setup').hidden = false;
  status('Choose a mode and trial count before starting another session. Treat repeated comparisons as exploratory.');
};

document.addEventListener('keydown', (event) => {
  if (!ready || busy || session?.status !== 'active' || event.ctrlKey || event.altKey || event.metaKey || event.target.closest('input,textarea,select,[contenteditable=true]')) return;
  if (event.code === 'Space') { event.preventDefault(); $('play').click(); }
  else if (['1', '2', '3'].includes(event.key)) {
    const slot = ['A', 'B', 'X'][Number(event.key) - 1];
    if (slot in session.trial.audio) { event.preventDefault(); transport.select(slot); }
  } else if (event.code === 'Home') { event.preventDefault(); transport.seek(transport.snapshot().start); }
  else if (event.code === 'Escape') transport.pause();
});
window.addEventListener('beforeunload', () => transport.destroy());

async function initialize() {
  config = await api('/api/config');
  $('title').textContent = config.title;
  document.title = `${config.title} · Muz blind comparator`;
  $('matching').textContent = config.level_matched ? 'LEVEL MATCHED · LOCAL' : 'LOCAL';
  history = saved('history', []);
  renderHistory();
  const previous = saved('session', null);
  if (previous) {
    try {
      session = await api(`/api/sessions/${encodeURIComponent(previous)}`);
      remember();
      await showSession();
      return;
    } catch (error) {
      status(`Could not resume the saved session: ${error.message}. You can start a new one.`, true);
    }
  } else status('Pair ready. Set the trial count, then choose a passage and compare blind.');
  $('setup').hidden = false;
}
requestAnimationFrame(frame);
guarded(initialize);
