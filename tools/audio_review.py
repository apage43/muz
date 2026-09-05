"""Development-only waveform/spectral review; never a composition or mastering stage.

uv run --project tools python tools/audio_review.py input.wav --output out/review
Sections: --section name:start_seconds:end_seconds (repeatable).
"""
import argparse
import json
import subprocess
from pathlib import Path

import matplotlib
matplotlib.use('Agg')
import matplotlib.pyplot as plt
import numpy as np

parser = argparse.ArgumentParser(description=__doc__)
parser.add_argument('audio', type=Path)
parser.add_argument('--output', required=True, type=Path)
parser.add_argument('--section', action='append', default=[])
a = parser.parse_args()
a.output.mkdir(parents=True, exist_ok=True)
rate = 48000
raw = subprocess.check_output(['ffmpeg','-v','error','-i',str(a.audio),'-f','f32le','-ar',str(rate),'-ac','2','-'])
x = np.frombuffer(raw, dtype='<f4').reshape(-1,2).astype(np.float64)
def db(v): return float(20*np.log10(max(float(v),1e-12)))
def metrics(y):
    mid = y.mean(axis=1)
    side = (y[:,0]-y[:,1])/2
    frames = min(len(mid)//8192, 5000)
    z = mid[:frames*8192].reshape(frames,8192)
    power = np.mean(np.abs(np.fft.rfft(z*np.hanning(8192),axis=1))**2,axis=0)
    hz = np.fft.rfftfreq(8192,1/rate)
    return dict(rms_dbfs=db(np.sqrt(np.mean(y*y))),peak_dbfs=db(np.max(np.abs(y))),
        correlation=float(np.corrcoef(y.T)[0,1]),
        side_to_mid_db=db(np.sqrt(np.mean(side*side))/max(np.sqrt(np.mean(mid*mid)),1e-12)),
        band_power_share={f'{lo}-{hi}':float(power[(hz>=lo)&(hz<hi)].sum()/max(power.sum(),1e-20))
            for lo,hi in [(25,120),(120,500),(500,2000),(2000,8000),(8000,20000)]})
sections = [item.split(':') for item in a.section]
report = dict(audio=str(a.audio.resolve()),seconds=len(x)/rate,overall=metrics(x),
    tail_last_second_rms_dbfs=db(np.sqrt(np.mean(x[-rate:]**2))),
    sections={name:metrics(x[int(float(start)*rate):int(float(end)*rate)]) for name,start,end in sections})
(a.output/'spectral-review.json').write_text(json.dumps(report,indent=2)+'\n')
plt.style.use('dark_background')
fig, axes = plt.subplots(3,1,figsize=(15,10),layout='constrained',height_ratios=[1,2,1])
step=480
chunks=x[:len(x)//step*step].reshape(-1,step,2)
t=np.arange(len(chunks))*step/rate
axes[0].fill_between(t,-np.max(np.abs(chunks),axis=(1,2)),np.max(np.abs(chunks),axis=(1,2)),color='#61c9dd',alpha=.6)
axes[0].plot(t,np.sqrt(np.mean(chunks*chunks,axis=(1,2))),color='#ffc174',lw=.7)
axes[0].set(ylabel='Peak / RMS',title=a.audio.name,xlim=(0,len(x)/rate))
axes[1].specgram(x.mean(axis=1),NFFT=4096,Fs=rate,noverlap=3072,cmap='magma',vmin=-110,vmax=-30)
axes[1].set(yscale='log',ylim=(25,18000),ylabel='Frequency (Hz)')
axes[2].plot(t,20*np.log10(np.maximum(np.sqrt(np.mean(chunks.mean(axis=2)**2,axis=1)),1e-10)),label='Mono mid',color='#61c9dd')
axes[2].plot(t,20*np.log10(np.maximum(np.sqrt(np.mean(((chunks[:,:,0]-chunks[:,:,1])/2)**2,axis=1)),1e-10)),label='Stereo side',color='#ffc174',alpha=.75)
axes[2].set(ylim=(-75,0),ylabel='RMS dBFS',xlabel='Seconds')
axes[2].legend(loc='upper right')
for name,start,end in sections:
    for ax in axes: ax.axvline(float(start),color='white',alpha=.2,lw=.7)
    axes[0].text(float(start)+.1,.9,name,fontsize=8,rotation=30)
fig.savefig(a.output/'review.png',dpi=120)
print(json.dumps(report,indent=2))
