#!/usr/bin/env python3
"""Compare paired actual-articulation filter transfer without fitting host gain.
Requires numpy. Input WAVs must be PCM16 with equal sample rate/channel count.
Low-energy bands remain explicit diagnostics; the tool does not infer conformance.
"""
import argparse
import json
import wave
from pathlib import Path
import numpy as np


def spectrum(path, start, end, size):
    with wave.open(str(path)) as source:
        if source.getsampwidth() != 2:
            raise ValueError('PCM16 input required')
        rate, channels = source.getframerate(), source.getnchannels()
        samples = np.frombuffer(source.readframes(source.getnframes()), dtype='<i2').astype(float).reshape(-1, channels)
    window = samples[round(start*rate):round(end*rate)] / 32768
    segments = []
    for offset in range(0, len(window)-size+1, size//2):
        segment = window[offset:offset+size]
        segment = segment-segment.mean(axis=0)
        segments.append(abs(np.fft.rfft(segment*np.hanning(size)[:, None], axis=0))**2)
    if not segments:
        raise ValueError('measurement window shorter than FFT segment')
    return rate, channels, np.fft.rfftfreq(size, 1/rate), np.mean(segments, axis=0).sum(axis=1)


def compare(paths, start=.25, end=.5, size=2048):
    values = [spectrum(path, start, end, size) for path in paths]
    if len({(v[0], v[1]) for v in values}) != 1:
        raise ValueError('equal rates/channels required')
    frequency = values[0][2]
    nf, af, npower, apower = [v[3] for v in values]
    rows = []
    for low, high in [(80,160),(160,320),(320,640),(640,1280),(1280,2560),(2560,5120),(5120,10240)]:
        mask = (frequency >= low) & (frequency < high)
        powers = [float(v[mask].sum()) for v in (nf,af,npower,apower)]
        if min(powers) <= 0:
            rows.append({'hz':[low,high], 'classification':'zero-energy comparison undefined'})
            continue
        nt, at = powers[0]/powers[2], powers[1]/powers[3]
        fraction = powers[2]/float(npower.sum())
        rows.append({'hz':[low,high], 'native_transfer_db':float(10*np.log10(nt)),
                     'aria_transfer_db':float(10*np.log10(at)), 'difference_db':float(10*np.log10(nt/at)),
                     'plain_native_power_fraction':fraction,
                     'classification':'low-energy diagnostic; attribution unresolved' if fraction < 1e-4 else 'energetic-band diagnostic; apply declared component gate'})
    return {'method':'summed-stereo power, Hann segments,50% overlap,segment mean removed; each host filtered/plain; no gain fit',
            'start_seconds':start,'end_seconds':end,'fft_frames':size,'bands':rows,
            'max_abs_band_db':max(abs(row['difference_db']) for row in rows if 'difference_db' in row)}

if __name__ == '__main__':
    parser = argparse.ArgumentParser(description=__doc__)
    for name in ['native-filtered','aria-filtered','native-plain','aria-plain']:
        parser.add_argument('--'+name, type=Path, required=True)
    parser.add_argument('--start', type=float, default=.25)
    parser.add_argument('--end', type=float, default=.5)
    parser.add_argument('--fft-frames', type=int, default=2048)
    parser.add_argument('--output', type=Path)
    args = parser.parse_args()
    result = compare([args.native_filtered,args.aria_filtered,args.native_plain,args.aria_plain],args.start,args.end,args.fft_frames)
    text=json.dumps(result,indent=2)+'\n'
    if args.output: args.output.write_text(text)
    else: print(text,end='')
