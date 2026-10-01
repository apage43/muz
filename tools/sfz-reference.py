#!/usr/bin/env python3
"""Synthetic installed-sfizz evidence; never downloads/executes a player.

Explicit --player, --library, and optional --muz select existing local tools.
Outputs are temporary unless --output is supplied. Read report.json as the
canonical JSON result; the installed library emits its own stream diagnostics. Unknown-opcode acceptance is NOT
proof of audible semantics. No external sample library is copied into git.
"""
import argparse
import ctypes
import hashlib
import json
import math
from pathlib import Path
import shutil
import struct
import subprocess
import tempfile
import wave


def vlq(value):
    out = [value & 127]
    while value > 127:
        value >>= 7
        out.insert(0, (value & 127) | 128)
    return bytes(out)


def midi(path):
    # 480 PPQ, 120 BPM; note at 0.125s, release at 0.625s, EOT at 1s.
    track = b'\x00\xff\x51\x03\x07\xa1\x20'
    track += vlq(120) + bytes([0x90, 60, 100])
    track += vlq(480) + bytes([0x80, 60, 64])
    track += vlq(360) + b'\xff\x2f\x00'
    path.write_bytes(b'MThd' + struct.pack('>IHHH', 6, 0, 1, 480)
                    + b'MTrk' + struct.pack('>I', len(track)) + track)


def sample(path, frequency=440):
    with wave.open(str(path), 'wb') as wav:
        wav.setparams((1, 2, 48000, 0, 'NONE', 'not compressed'))
        wav.writeframes(b''.join(struct.pack('<h', round(8000 * math.sin(2 * math.pi * frequency * i / 48000)))
                                 for i in range(48000)))


def samples(path):
    data = path.read_bytes()
    if data[:4] != b'RIFF' or data[8:12] != b'WAVE':
        raise ValueError('expected RIFF WAVE')
    pos, fmt, payload = 12, None, None
    while pos + 8 <= len(data):
        kind, length = data[pos:pos+4], struct.unpack_from('<I', data, pos+4)[0]
        chunk = data[pos+8:pos+8+length]
        if kind == b'fmt ': fmt = struct.unpack_from('<HHIIHH', chunk)
        if kind == b'data': payload = chunk
        pos += 8 + length + (length & 1)
    if fmt is None or payload is None: raise ValueError('missing WAV chunks')
    tag, channels, rate, _, _, bits = fmt
    if tag == 3 and bits == 32:
        values = struct.unpack('<' + 'f' * (len(payload)//4), payload)
    elif tag == 1 and bits == 16:
        values = [v/32768 for v in struct.unpack('<' + 'h' * (len(payload)//2), payload)]
    else: raise ValueError(f'unsupported WAV format {tag}/{bits}')
    return channels, rate, values


def metrics(path):
    channels, rate, values = samples(path)
    def rms(a, b):
        segment = values[round(a*rate)*channels:round(b*rate)*channels]
        return math.sqrt(sum(v*v for v in segment)/max(1, len(segment)))
    return dict(channels=channels, rate=rate, frames=len(values)//channels,
                peak=max(map(abs, values), default=0), sustain_rms=rms(.25,.5),
                release_rms=rms(.7,.9), sha256=hashlib.sha256(path.read_bytes()).hexdigest())


def pitch_trace(path, root_hz=440.0):
    """Positive zero-crossing pitch samples; use for clean synthetic sine only."""
    channels, rate, values = samples(path)
    mono = [sum(values[i:i+channels])/channels for i in range(0,len(values),channels)]
    crossings = [i-mono[i]/(mono[i+1]-mono[i]) for i in range(len(mono)-1)
                 if mono[i] <= 0 < mono[i+1] and mono[i+1] != mono[i]]
    return [{'time_seconds':(a+b)*0.5/rate,
             'pitch_cents':1200*math.log2(rate/(b-a)/root_hz)}
            for a,b in zip(crossings,crossings[1:])
            if b>a and 0.135 <= (a+b)*0.5/rate <= 0.62]


def feature_metrics(path):
    channels, rate, values = samples(path)
    mono = values[::channels]
    crossings = []
    for i in range(round(.25*rate), round(.5*rate)-1):
        if mono[i] <= 0 < mono[i+1]:
            crossings.append(i-mono[i]/(mono[i+1]-mono[i]))
    frequency = ((len(crossings)-1)*rate/(crossings[-1]-crossings[0])) if len(crossings)>1 else None
    peak = max(mono[6000:30000], default=0)
    def first_crossing(start, stop, threshold, rising):
        for i in range(start,stop):
            if (mono[i] >= threshold if rising else mono[i] <= threshold):
                return i/rate
        return None
    return dict(frequency_hz=frequency,
                first_signal_seconds=next((i/rate for i,v in enumerate(mono) if abs(v)>1e-4), None),
                # Useful for DC fixtures; carrier-wave cases must not use these as envelope gates.
                dc_attack_90_seconds=first_crossing(6000,30000,.9*peak,True),
                dc_decay_50_seconds=first_crossing(6480,30000,.5*peak,False),
                dc_release_10_seconds=first_crossing(30000,48000,.1*peak,False),
                dc_release_1_seconds=first_crossing(30000,48000,.01*peak,False))


def compare(left, right, remove_reference_frame=None):
    """Report differences without silently aligning/gain-normalizing audio.

    Different frame lengths are explicit; common-prefix metrics are diagnostic,
    not a pass/fail verdict. Feature-specific acceptance tolerances belong in
    the calling test (filter/envelope differences cannot use one global bound).
    """
    lc, lr, a = samples(left)
    rc, rr, b = samples(right)
    if (lc, lr) != (rc, rr):
        raise ValueError('channel count/sample rate mismatch')
    if remove_reference_frame is not None:
        start = remove_reference_frame*lc
        a = a[:start]+a[start+lc:]
    count = min(len(a), len(b))
    errors = [x-y for x, y in zip(a, b)]
    rms_error = math.sqrt(sum(x*x for x in errors)/max(1, count))
    rms_reference = math.sqrt(sum(x*x for x in a[:count])/max(1, count))
    return dict(channels=lc, rate=lr, left_frames=len(a)//lc, right_frames=len(b)//lc,
                compared_frames=count//lc, max_absolute_error=max(map(abs, errors), default=0),
                rms_error=rms_error,
                relative_rms_error=rms_error/rms_reference if rms_reference else None)


PROBES = {
    'baseline': '',
    'lfo-sine': 'sample=dc.wav amp_veltrack=0 lfo01_freq=2 lfo01_volume=6 lfo01_wave=1',
    'lfo-triangle': 'sample=dc.wav amp_veltrack=0 lfo01_freq=2 lfo01_volume=6 lfo01_wave=0',
    'ramp': 'sample=ramp.wav ampeg_attack=0 ampeg_decay=0 ampeg_sustain=100 ampeg_release=0 amp_veltrack=0',
    'dc-attack': 'sample=dc.wav amp_veltrack=0 ampeg_attack=0.1',
    'dc-decay': 'sample=dc.wav amp_veltrack=0 ampeg_attack=0.01 ampeg_decay=0.2 ampeg_sustain=25',
    'dc-release': 'sample=dc.wav amp_veltrack=0 ampeg_release=0.2',
    'attack': 'ampeg_attack=0.1',
    'decay': 'ampeg_attack=0.01 ampeg_decay=0.2 ampeg_sustain=25',
    'release': 'ampeg_release=0.2',
    'continuous-loop': 'loop_mode=loop_continuous loop_start=1000 loop_end=5000',
    'sustain-loop': 'loop_mode=loop_sustain loop_start=1000 loop_end=5000',
    'offset': 'offset=1200',
    'transpose': 'transpose=12',
    'case-key': 'Key=61',
    'case-envelope': 'Ampeg_release=0.01',
    'case-bend': 'bend_Down=1200',
    'eg-level-negative': 'eg1_pitch=1200 eg1_level0=-100 eg1_time1=0.2 eg1_level1=0 eg1_sustain=1',
    'eg-level-unit': 'eg1_pitch=1200 eg1_level0=-1 eg1_time1=0.2 eg1_level1=0 eg1_sustain=1',
    'crossmod-alias': 'lfo03_freq=2 lfo03_pitch_oncc117=10 lfo03_freq_lfo2_oncc117=10',
    'var-multiply': 'cutoff=250 var01_cutoff=6000 var01_mod=mult var01_oncc131=1 var01_oncc92=1',
    'crossmod-full': 'lfo03_freq=2 lfo03_pitch_oncc117=10 lfo03_freq_lfo02_oncc117=10',
    'crossmod-base': 'lfo03_freq=2 lfo03_pitch_oncc117=10 lfo03_freq_lfo02=10',
    'virtual-velocity': 'volume_oncc131=12',
    'virtual-key': 'volume_oncc133=12',
    'virtual-random': 'volume_oncc135=12',
    'virtual-keydelta': 'eg1_pitch_oncc140=100 eg1_level0=-1 eg1_time1=0.2 eg1_level1=0 eg1_sustain=1',
}


def unknown(library, sfz):
    lib = ctypes.CDLL(library)
    lib.sfizz_create_synth.restype = ctypes.c_void_p
    lib.sfizz_load_file.argtypes = [ctypes.c_void_p, ctypes.c_char_p]
    lib.sfizz_load_file.restype = ctypes.c_bool
    lib.sfizz_get_unknown_opcodes.argtypes = [ctypes.c_void_p]
    lib.sfizz_get_unknown_opcodes.restype = ctypes.c_void_p
    lib.sfizz_free_memory.argtypes = [ctypes.c_void_p]
    lib.sfizz_free.argtypes = [ctypes.c_void_p]
    lib.sfizz_get_volume.argtypes = [ctypes.c_void_p]
    lib.sfizz_get_volume.restype = ctypes.c_float
    synth = lib.sfizz_create_synth()
    try:
        loaded = lib.sfizz_load_file(synth, str(sfz).encode())
        pointer = lib.sfizz_get_unknown_opcodes(synth)
        try: text = ctypes.string_at(pointer).decode() if pointer else ''
        finally:
            if pointer: lib.sfizz_free_memory(pointer)
        return dict(loaded=loaded, unknown=text, default_host_gain_db=lib.sfizz_get_volume(synth))
    finally: lib.sfizz_free(synth)


def api_render(library, sfz, output, block=48):
    """C API reference with explicit unity host gain and exact frame timing."""
    lib = ctypes.CDLL(library)
    ptr = ctypes.c_void_p
    lib.sfizz_create_synth.restype = ptr
    lib.sfizz_free.argtypes = [ptr]
    lib.sfizz_load_file.argtypes = [ptr, ctypes.c_char_p]
    lib.sfizz_load_file.restype = ctypes.c_bool
    lib.sfizz_set_samples_per_block.argtypes = [ptr, ctypes.c_int]
    lib.sfizz_set_sample_rate.argtypes = [ptr, ctypes.c_float]
    lib.sfizz_set_volume.argtypes = [ptr, ctypes.c_float]
    lib.sfizz_send_note_on.argtypes = [ptr, ctypes.c_int, ctypes.c_int, ctypes.c_int]
    lib.sfizz_send_note_off.argtypes = [ptr, ctypes.c_int, ctypes.c_int, ctypes.c_int]
    lib.sfizz_render_block.argtypes = [ptr, ctypes.POINTER(ctypes.POINTER(ctypes.c_float)), ctypes.c_int, ctypes.c_int]
    synth = lib.sfizz_create_synth()
    try:
        lib.sfizz_set_samples_per_block(synth, block)
        lib.sfizz_set_sample_rate(synth, 48000)
        lib.sfizz_set_volume(synth, 0)
        if not lib.sfizz_load_file(synth, str(sfz).encode()):
            raise ValueError('reference rejected program')
        buffers = [(ctypes.c_float * block)() for _ in range(2)]
        channels = (ctypes.POINTER(ctypes.c_float)*2)(*buffers)
        pcm = bytearray()
        for start in range(0, 48000, block):
            count = min(block, 48000-start)
            for frame, on in [(6000, True), (30000, False)]:
                if start <= frame < start+count:
                    if on: lib.sfizz_send_note_on(synth, frame-start, 60, 100)
                    else: lib.sfizz_send_note_off(synth, frame-start, 60, 64)
            lib.sfizz_render_block(synth, channels, 2, count)
            for i in range(count):
                for channel in buffers:
                    pcm.extend(struct.pack('<h', max(-32768, min(32767, round(channel[i]*32768)))))
        with wave.open(str(output), 'wb') as wav:
            wav.setparams((2,2,48000,0,'NONE','not compressed'))
            wav.writeframes(pcm)
    finally:
        lib.sfizz_free(synth)


def syntax_probes(root, library):
    """Observe permissive reference parsing; do not infer source corrections."""
    folder = root/'syntax'
    (folder/'maps'/'maps').mkdir(parents=True, exist_ok=True)
    sample(folder/'tone.wav')
    (folder/'maps'/'a.sfz').write_text('#include "maps/b.sfz"\n')
    (folder/'maps'/'b.sfz').write_text('<region> sample=tone.wav key=60\n')
    # Conflicting include-relative target proves root-relative priority.
    (folder/'maps'/'maps'/'b.sfz').write_text('<region> sample=../../tone.wav key=61\n')
    cases = {'root-include-priority': '#include "maps/a.sfz"\n',
             'sample-path-case': '<region> sample=Tone.wav key=60\n',
             'sample-path-case-upper': '<region> sample=TONE.WAV key=60\n',
             'bare-slash': '/ Cymbals & Tamtam\n<region> sample=tone.wav key=60\n',
             'bare-volume-token': '<group> volume-1\n<region> sample=tone.wav key=60\n',
             'bare-volume-token2': '<group> volume-2\n<region> sample=tone.wav key=60\n',
             'baseline': '<region> sample=tone.wav key=60\n'}
    report = {}
    for name, text in cases.items():
        sfz = folder/(name+'.sfz')
        sfz.write_text(text)
        output = folder/(name+'.wav')
        report[name] = unknown(library, sfz)
        api_render(library, sfz, output)
        report[name]['metrics'] = metrics(output)
    for name in cases:
        report[name]['baseline_comparison'] = compare(folder/'baseline.wav', folder/(name+'.wav'))
    return report


def polyphony_probes(root, library):
    """Actual layered group/key voice limits; no note-owner approximation."""
    folder = root/'polyphony'
    folder.mkdir(exist_ok=True)
    sample(folder/'tone.wav')
    lib = ctypes.CDLL(library)
    ptr = ctypes.c_void_p
    lib.sfizz_create_synth.restype = ptr
    for name, args, ret in [
        ('sfizz_free', [ptr], None),
        ('sfizz_set_samples_per_block', [ptr, ctypes.c_int], None),
        ('sfizz_load_file', [ptr, ctypes.c_char_p], ctypes.c_bool),
        ('sfizz_send_note_on', [ptr, ctypes.c_int, ctypes.c_int, ctypes.c_int], None),
        ('sfizz_get_num_active_voices', [ptr], ctypes.c_int),
        ('sfizz_render_block', [ptr, ctypes.POINTER(ctypes.POINTER(ctypes.c_float)), ctypes.c_int, ctypes.c_int], None),
    ]:
        func = getattr(lib, name)
        func.argtypes, func.restype = args, ret
    report = {}
    cases = [(str(limit), limit, False, 2, [100,100,100]) for limit in (None,0,1,2,3,4)]
    cases.append(('selfmask-louder',1,True,1,[127,64,64]))
    for name, limit, mask, layers, velocities in cases:
        sfz = folder/(name+'.sfz')
        poly = f'note_polyphony={limit} ' if limit is not None else ''
        sfz.write_text('<group> group=1 '+poly+'note_selfmask='+('on' if mask else 'off')+' '
                       'ampeg_release=0.001 off_time=0.001\n'
                       +'<region> key=60 sample=tone.wav\n'*layers)
        synth = lib.sfizz_create_synth()
        try:
            lib.sfizz_set_samples_per_block(synth, 64)
            if not lib.sfizz_load_file(synth, str(sfz).encode()):
                raise ValueError('reference rejected polyphony fixture')
            buffers = [(ctypes.c_float*64)() for _ in range(2)]
            channels = (ctypes.POINTER(ctypes.c_float)*2)(*buffers)
            trace = []
            for velocity in velocities:
                lib.sfizz_send_note_on(synth, 0, 60, velocity)
                for _ in range(100):
                    lib.sfizz_render_block(synth, channels, 2, 64)
                trace.append(lib.sfizz_get_num_active_voices(synth))
            report[name] = dict(limit=limit, selfmask=mask, layers=layers, velocities=velocities, active_voices=trace)
        finally:
            lib.sfizz_free(synth)
    return report


def amplitude_probes(root, library):
    """Observe authored base/CC product and implicit-controller replacement."""
    folder = root/'amplitude'
    folder.mkdir(exist_ok=True)
    sample(folder/'tone.wav')
    report = {}
    cases = {}
    for base in (0,100):
        for cc in (0,64,127):
            cases[f'base{base}-cc{cc}'] = (
                f'set_cc7=127 set_cc11=127 set_cc12={cc}',
                f'amplitude={base} amplitude_oncc12=100')
    for base in (0,100):
        cases[f'two-routes-base{base}'] = (
            'set_cc7=64 set_cc11=127 set_cc107=64',
            f'amplitude={base} amplitude_oncc7=100 amplitude_oncc107=100')
    for name, (control, opcodes) in cases.items():
        sfz = folder/(name+'.sfz')
        sfz.write_text('<control> '+control+'\n<region> key=60 sample=tone.wav amp_veltrack=0 '+opcodes+'\n')
        output = folder/(name+'.wav')
        api_render(library, sfz, output)
        report[name] = dict(**unknown(library, sfz), **metrics(output))
    return report


def run(root, player, library, muz=None):
    root.mkdir(parents=True, exist_ok=True)
    sample(root/'tone.wav'); midi(root/'notes.mid')
    with wave.open(str(root/'ramp.wav'), 'wb') as wav:
        wav.setparams((1,2,48000,0,'NONE','not compressed'))
        wav.writeframes(b''.join(struct.pack('<h', i*8 if i<1000 else 0) for i in range(48000)))
    with wave.open(str(root/'dc.wav'), 'wb') as wav:
        wav.setparams((1,2,48000,0,'NONE','not compressed'))
        wav.writeframes(struct.pack('<h', 5000)*48000)
    result = {'player': str(Path(player).resolve()), 'player_sha256': hashlib.sha256(Path(player).read_bytes()).hexdigest(),
              'library': library, 'library_sha256': hashlib.sha256(Path(library).read_bytes()).hexdigest(), 'scope': 'synthetic probes only; no corpus/reference conformance claim',
              'reference_notes': [
                  'CLI default sfizz host gain observed -7.35dB; API renders explicitly use0dB.',
                  'API/fixture CC7 and11 explicitly127; baseline controller routing measured separately.',
                  'sfizz1.2.3 first callback repeats last source sample once; ramp fixture exposes this.',
                  'sfizz1.2.3 enforces at least1ms loop_crossfade; default source looping comparison includes this deviation.',
                  'Raw comparisons preserve exact scheduling/gain; named startup diagnostic removes only observed repeatframe.',
              ], 'probes': {}}
    if muz:
        result['muz'] = dict(path=str(Path(muz).resolve()), sha256=hashlib.sha256(Path(muz).read_bytes()).hexdigest())
    for name, opcodes in PROBES.items():
        sfz = root/(name+'.sfz')
        sfz.write_text('<control> set_cc7=127 set_cc11=127 set_cc117=127\n<region> sample=tone.wav key=60 ampeg_release=0.1 '+opcodes+'\n')
        entry = unknown(library, sfz)
        api_output = root/(name+'-api.wav')
        api_render(library, sfz, api_output)
        entry['api'] = dict(block=48, master_gain_db=0, cc7=127, cc11=127, **metrics(api_output))
        entry['api']['features'] = feature_metrics(api_output)
        entry['renders'] = []
        for block in (64, 256):
            output = root/f'{name}-{block}.wav'
            proc = subprocess.run([player, '--sfz', str(sfz), '--midi', str(root/'notes.mid'), '--wav', str(output),
                                   '--samplerate', '48000', '--blocksize', str(block), '--use-eot'], capture_output=True, text=True)
            render = dict(block=block, exit=proc.returncode, stderr=proc.stderr)
            if proc.returncode == 0: render.update(metrics(output))
            entry['renders'].append(render)
        if muz:
            source = root/(name+'.muz')
            source.write_text('song({tempo:120,tail:0,tracks:[track("reference",'
                              'note(60,1b,at=0.25b,velocity=100/127).gate(1),'
                              'sfz('+json.dumps(str(sfz))+'),{gain:0})]})\n')
            output = root/(name+'-muz.wav')
            proc = subprocess.run([muz, 'render', str(source), '-o', str(output),
                                   '--sample-rate', '48000', '--seconds', '1', '--tail', '0',
                                   '--format', 'pcm16'], capture_output=True, text=True)
            entry['muz'] = dict(exit=proc.returncode, stderr=proc.stderr)
            if proc.returncode == 0:
                entry['muz']['metrics'] = metrics(output)
                entry['muz']['features'] = feature_metrics(output)
                ref_rms, native_rms = entry['api']['sustain_rms'], entry['muz']['metrics']['sustain_rms']
                entry['muz']['sustain_gain_difference_db'] = 20*math.log10(native_rms/ref_rms) if ref_rms and native_rms else None
                ref_hz, native_hz = entry['api']['features']['frequency_hz'], entry['muz']['features']['frequency_hz']
                entry['muz']['sustain_pitch_difference_cents'] = 1200*math.log2(native_hz/ref_hz) if ref_hz and native_hz else None
                entry['muz']['comparison'] = compare(api_output, output)
                entry['muz']['comparison']['reference_supports_all_opcodes'] = not bool(entry['unknown'])
                entry['muz']['sfizz_first_callback_repeat_diagnostic'] = dict(
                    removed_reference_frame=6048,
                    reason='installed sfizz ramp probe repeats sample at first callback boundary; raw comparison retained',
                    **compare(api_output, output, remove_reference_frame=6048))
        result['probes'][name] = entry
    result['amplitude_routes'] = amplitude_probes(root, library)
    result['syntax'] = syntax_probes(root, library)
    result['layered_note_polyphony'] = polyphony_probes(root, library)
    (root/'report.json').write_text(json.dumps(result, indent=2)+'\n')
    return result


if __name__ == '__main__':
    args = argparse.ArgumentParser(description=__doc__)
    args.add_argument('--player')
    args.add_argument('--library')
    args.add_argument('--compare', type=Path, nargs=2, metavar=('REFERENCE_WAV', 'MUZ_WAV'))
    args.add_argument('--output', type=Path)
    args.add_argument('--muz', help='explicit existing muz CLI; render same synthetic notes for diagnostic comparison')
    options = args.parse_args()
    if options.compare:
        print(json.dumps(compare(*options.compare), indent=2))
        raise SystemExit(0)
    if not options.player or not options.library:
        args.error('rendering requires explicit --player and --library')
    if options.output:
        report = run(options.output.resolve(), options.player, options.library, options.muz)
    else:
        with tempfile.TemporaryDirectory(prefix='muz-sfz-reference-') as directory:
            report = run(Path(directory), options.player, options.library, options.muz)
    print(json.dumps(report, indent=2))
