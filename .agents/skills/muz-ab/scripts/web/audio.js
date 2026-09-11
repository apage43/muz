const SWITCH_SECONDS = 0.005;
const LOOP_FADE_SECONDS = 0.003;
const MIN_REGION_SECONDS = 0.02;
const START_DELAY_SECONDS = 0.01;
const FADE_LOOKAHEAD_SECONDS = 3;

const clamp = (value, low, high) => Math.max(low, Math.min(high, value));

function finiteNumber(value, name) {
  if (typeof value !== "number" || !Number.isFinite(value)) {
    throw new TypeError(`${name} must be a finite number`);
  }
  return value;
}

export class AudioTransport {
  #context = null;
  #master = null;
  #buffers = new Map();
  #group = null;
  #abort = null;
  #generation = 0;
  #playRequest = 0;
  #duration = 0;
  #frames = 0;
  #position = 0;
  #playing = false;
  #selected = "A";
  #loop = true;
  #start = 0;
  #end = 0;
  #volume = 1;
  #listened = { A: 0, B: 0, X: 0 };
  #switches = 0;
  #accountedAt = 0;
  #waveform = null;

  async load(audioURLMap, { sampleRate } = {}) {
    const generation = ++this.#generation;
    this.#release();
    this.#reset();
    const slots = Object.keys(audioURLMap ?? {});
    if (!slots.includes("A") || !slots.includes("B") ||
        slots.some(slot => !["A", "B", "X"].includes(slot)) ||
        slots.some(slot => typeof audioURLMap[slot] !== "string" || !audioURLMap[slot])) {
      throw new TypeError("Audio must provide A and B URLs, and optionally X");
    }
    if (!Number.isInteger(sampleRate) || sampleRate <= 0) {
      throw new TypeError("sampleRate must be a positive integer");
    }

    const context = new AudioContext({ sampleRate });
    this.#context = context;
    const abort = new AbortController();
    this.#abort = abort;
    try {
      if (context.sampleRate !== sampleRate) {
        throw new Error("The audio device cannot use the required sample rate");
      }
      const decoded = await Promise.all(slots.map(async slot => {
        const response = await fetch(audioURLMap[slot], { signal: abort.signal });
        if (!response.ok) throw new Error(`Audio request failed (${response.status})`);
        const buffer = await context.decodeAudioData(await response.arrayBuffer());
        return [slot, buffer];
      }));
      if (generation !== this.#generation) {
        throw new DOMException("Audio load was replaced", "AbortError");
      }
      const reference = decoded[0][1];
      const lengths = decoded.map(([, buffer]) => buffer.length);
      const frames = Math.min(...lengths);
      if (!frames || Math.max(...lengths) - frames > 1 || decoded.some(([, buffer]) =>
        buffer.sampleRate !== sampleRate ||
        buffer.numberOfChannels !== reference.numberOfChannels)) {
        throw new Error("Candidate audio is not sample-aligned");
      }
      this.#buffers = new Map(decoded);
      this.#frames = frames;
      this.#duration = frames / sampleRate;
      this.#end = this.#duration;
      this.#master = context.createGain();
      this.#master.gain.value = this.#volume;
      this.#master.connect(context.destination);
      this.#abort = null;
    } catch (error) {
      if (generation === this.#generation) {
        this.#release();
        this.#reset();
      }
      throw error;
    }
  }

  async play() {
    this.#requireLoaded();
    const context = this.#context;
    const request = ++this.#playRequest;
    const generation = this.#generation;
    await context.resume();
    if (generation !== this.#generation || request !== this.#playRequest) return;
    if (context.state !== "running") throw new Error("Audio playback is suspended");
    this.#account();
    if (this.#playing) return;
    if (!this.#loop && this.#position >= this.#duration) this.#position = 0;
    this.#startSources(this.#normalizePosition(this.#position));
  }

  pause() {
    ++this.#playRequest;
    this.#account();
    this.#position = this.#currentPosition();
    this.#playing = false;
    this.#stopSources();
  }

  seek(seconds) {
    this.#requireLoaded();
    finiteNumber(seconds, "Position");
    this.#account();
    this.#replacePosition(this.#normalizePosition(seconds));
  }

  select(slot) {
    this.#requireLoaded();
    if (!this.#buffers.has(slot)) throw new RangeError("Unknown listening slot");
    this.#account();
    if (slot === this.#selected) return;
    this.#selected = slot;
    ++this.#switches;
    const group = this.#group;
    if (!group) return;
    const now = this.#context.currentTime;
    // Hold the interpolated value, not AudioParam.value: rapid switches can
    // interrupt a previous fade. All gain sums remain one throughout a fade.
    const fraction = clamp((now - group.mixStart) / SWITCH_SECONDS, 0, 1);
    for (const [name, node] of group.nodes) {
      const current = node.from + (node.to - node.from) * fraction;
      const target = name === slot ? 1 : 0;
      node.gain.gain.cancelScheduledValues(now);
      node.gain.gain.setValueAtTime(current, now);
      node.gain.gain.linearRampToValueAtTime(target, now + SWITCH_SECONDS);
      node.from = current;
      node.to = target;
    }
    group.mixStart = now;
  }

  setRegion(start, end) {
    this.#requireLoaded();
    finiteNumber(start, "Region start");
    finiteNumber(end, "Region end");
    const rate = this.#context.sampleRate;
    let first = clamp(Math.round(start * rate), 0, this.#frames);
    let last = clamp(Math.round(end * rate), 0, this.#frames);
    if (last < first) [first, last] = [last, first];
    const minimum = Math.min(this.#frames, Math.ceil(MIN_REGION_SECONDS * rate));
    if (last - first < minimum) {
      last = Math.min(this.#frames, first + minimum);
      first = last - minimum;
    }
    const nextStart = first / rate;
    const nextEnd = last / rate;
    if (nextStart === this.#start && nextEnd === this.#end) return;
    this.#account();
    const position = this.#currentPosition();
    this.#start = nextStart;
    this.#end = nextEnd;
    if (this.#loop) this.#replacePosition(this.#normalizePosition(position));
  }

  setLoop(enabled) {
    this.#requireLoaded();
    if (typeof enabled !== "boolean") throw new TypeError("Loop must be boolean");
    if (enabled === this.#loop) return;
    this.#account();
    const position = this.#currentPosition();
    this.#loop = enabled;
    this.#replacePosition(this.#normalizePosition(position));
  }

  setVolume(volume) {
    finiteNumber(volume, "Volume");
    if (volume < 0 || volume > 1) throw new RangeError("Volume must be between 0 and 1");
    this.#account();
    this.#volume = volume;
    if (this.#master) {
      const gain = this.#master.gain;
      const now = this.#context.currentTime;
      gain.cancelAndHoldAtTime(now);
      gain.linearRampToValueAtTime(volume, now + SWITCH_SECONDS);
    }
  }

  waveform(bins) {
    this.#requireLoaded();
    if (!Number.isInteger(bins) || bins < 1 || bins > 65536) {
      throw new RangeError("Waveform bins must be an integer between 1 and 65536");
    }
    if (this.#waveform?.length === bins) return this.#waveform;
    const peaks = new Float32Array(bins);
    // A shared absolute-peak envelope cannot identify which slot contains a
    // transient. The UI draws each peak symmetrically about its center line.
    for (const buffer of this.#buffers.values()) {
      for (let channel = 0; channel < buffer.numberOfChannels; ++channel) {
        const data = buffer.getChannelData(channel);
        for (let bin = 0; bin < bins; ++bin) {
          const first = Math.floor(bin * this.#frames / bins);
          const last = Math.min(this.#frames, Math.max(first + 1,
            Math.floor((bin + 1) * this.#frames / bins)));
          let peak = peaks[bin];
          for (let frame = first; frame < last; ++frame) {
            peak = Math.max(peak, Math.abs(data[frame]));
          }
          peaks[bin] = peak;
        }
      }
    }
    this.#waveform = peaks;
    return peaks;
  }

  snapshot() {
    this.#account();
    return {
      duration: this.#duration,
      position: this.#currentPosition(),
      playing: this.#playing && this.#context?.state === "running",
      selected: this.#selected,
      loop: this.#loop,
      start: this.#start,
      end: this.#end,
      volume: this.#volume,
      listened_seconds: { ...this.#listened },
      switches: this.#switches,
    };
  }

  destroy() {
    ++this.#generation;
    this.#release();
    this.#reset();
  }

  #requireLoaded() {
    if (!this.#context || !this.#buffers.size) throw new Error("No audio is loaded");
  }

  #reset() {
    this.#duration = this.#frames = this.#position = this.#start = this.#end = 0;
    this.#selected = "A";
    this.#loop = true;
    this.#volume = 1;
    this.#listened = { A: 0, B: 0, X: 0 };
    this.#switches = this.#accountedAt = 0;
    this.#waveform = null;
  }

  #release() {
    ++this.#playRequest;
    this.#playing = false;
    this.#stopSources();
    this.#abort?.abort();
    this.#abort = null;
    this.#buffers.clear();
    this.#master?.disconnect();
    this.#master = null;
    if (this.#context && this.#context.state !== "closed") {
      void this.#context.close().catch(() => {});
    }
    this.#context = null;
  }

  #normalizePosition(seconds) {
    const position = clamp(Math.round(seconds * this.#context.sampleRate) /
      this.#context.sampleRate, 0, this.#duration);
    if (!this.#loop) return position;
    return position < this.#start || position >= this.#end ? this.#start : position;
  }

  #currentPosition() {
    const group = this.#group;
    if (!this.#playing || !group) return this.#position;
    const elapsed = Math.max(0, this.#context.currentTime - group.when);
    const position = group.offset + elapsed;
    if (!group.loop) return Math.min(this.#duration, position);
    return this.#start + ((position - this.#start) % (this.#end - this.#start));
  }

  #account() {
    const group = this.#group;
    if (!this.#playing || !group) return;
    const now = this.#context.currentTime;
    const until = group.loop ? now : Math.min(now, group.endsAt);
    const elapsed = Math.max(0, until - this.#accountedAt);
    if (this.#volume > 0) this.#listened[this.#selected] += elapsed;
    this.#accountedAt = Math.max(this.#accountedAt, until);
    if (!group.loop && now >= group.endsAt) {
      this.#position = this.#duration;
      this.#playing = false;
      this.#stopSources();
    }
  }

  #replacePosition(position) {
    const wasPlaying = this.#playing;
    this.#playing = false;
    this.#stopSources();
    this.#position = position;
    if (wasPlaying && (this.#loop || position < this.#duration)) {
      this.#startSources(position);
    }
  }

  #startSources(offset) {
    const context = this.#context;
    const when = context.currentTime + START_DELAY_SECONDS;
    const remaining = (this.#loop ? this.#end : this.#duration) - offset;
    const edgeFade = Math.min(LOOP_FADE_SECONDS, (this.#end - this.#start) / 4);
    const group = {
      nodes: new Map(),
      envelope: context.createGain(),
      when,
      offset,
      loop: this.#loop,
      endsAt: when + this.#duration - offset,
      boundary: when + remaining,
      firstBoundary: when + remaining,
      period: this.#end - this.#start,
      edgeFade,
      mixStart: when - SWITCH_SECONDS,
      timer: null,
    };
    const envelope = group.envelope.gain;
    envelope.setValueAtTime(0, when);
    envelope.linearRampToValueAtTime(1, when + Math.min(edgeFade, remaining / 4));
    group.envelope.connect(this.#master);
    this.#group = group;
    this.#position = offset;
    this.#accountedAt = when;
    this.#playing = true;
    try {
      for (const [slot, buffer] of this.#buffers) {
        const source = context.createBufferSource();
        const gain = context.createGain();
        const value = slot === this.#selected ? 1 : 0;
        gain.gain.value = value;
        source.buffer = buffer;
        source.loop = group.loop;
        source.loopStart = this.#start;
        source.loopEnd = this.#end;
        source.connect(gain);
        gain.connect(group.envelope);
        group.nodes.set(slot, { source, gain, from: value, to: value });
        source.onended = () => {
          if (this.#group === group && !group.loop) this.#account();
        };
        source.start(when, offset);
        if (!group.loop) source.stop(group.endsAt);
      }
      if (group.loop && group.period >= MIN_REGION_SECONDS) {
        this.#scheduleLoopFades(group);
        group.timer = setInterval(() => this.#scheduleLoopFades(group), 250);
      } else if (!group.loop) {
        envelope.setValueAtTime(1, group.endsAt - Math.min(edgeFade, remaining / 4));
        envelope.linearRampToValueAtTime(0, group.endsAt);
      }
    } catch (error) {
      this.#playing = false;
      this.#stopSources();
      throw error;
    }
  }

  #scheduleLoopFades(group) {
    if (this.#group !== group) return;
    const now = this.#context.currentTime;
    // The sources loop natively: this timer only schedules a shared edge taper,
    // never playback or loop timing. If a browser freezes timers for >3 seconds,
    // looping stays aligned but unsmoothed arbitrary endpoints can click.
    // Clips shorter than the minimum selectable region skip recurring tapers:
    // sub-20ms gain modulation is audible and tiny loops can flood automation.
    if (group.boundary - group.edgeFade <= now) {
      group.boundary += (Math.floor((now + group.edgeFade - group.boundary) /
        group.period) + 1) * group.period;
    }
    const horizon = now + FADE_LOOKAHEAD_SECONDS;
    while (group.boundary <= horizon) {
      const fade = group.boundary === group.firstBoundary
        ? Math.min(group.edgeFade, (group.boundary - group.when) / 4)
        : group.edgeFade;
      const gain = group.envelope.gain;
      gain.setValueAtTime(1, group.boundary - fade);
      gain.linearRampToValueAtTime(0, group.boundary);
      gain.linearRampToValueAtTime(1, group.boundary + fade);
      group.boundary += group.period;
    }
  }

  #stopSources() {
    const group = this.#group;
    if (!group) return;
    this.#group = null;
    clearInterval(group.timer);
    for (const { source, gain } of group.nodes.values()) {
      source.onended = null;
      try { source.stop(); } catch { /* A source can already have ended. */ }
      source.disconnect();
      gain.disconnect();
    }
    group.envelope.disconnect();
  }
}
