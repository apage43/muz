#!/usr/bin/env python3
"""Measure isolated DC volume-LFO clocks against source-N -> target-X additive Hz.
Input files w{0|1}-f{2|4}-d{depth}.wav come from the synthetic ARIA clock suite.
Reports cycle-time gates separately; large negative-rate probes remain diagnostics.
"""
import argparse,importlib.util,json,math,re
from pathlib import Path
sp=importlib.util.spec_from_file_location('reference',Path(__file__).with_name('sfz-reference.py'))
r=importlib.util.module_from_spec(sp);sp.loader.exec_module(r)
def integral(t,wave):
 t=t%1
 if wave==1:return (1-math.cos(2*math.pi*t))/(2*math.pi)
 if t<.25:return 2*t*t
 if t<.75:return 2*t-2*t*t-.25
 return 2*t*t-4*t+2

def analyze(path):
 m=re.fullmatch(r'w([01])-f([24])-d([0-9.]+)',path.stem)
 wave,base,depth=int(m[1]),float(m[2]),float(m[3])
 channels,rate,values=r.samples(path)
 mono=[sum(values[i:i+channels])/channels for i in range(0,len(values),channels)]
 mono=mono[6000:126000]
 center=math.sqrt(min(mono)*max(mono))
 db=[20*math.log10(x/center) for x in mono]
 crossings=[(i-db[i]/(db[i+1]-db[i]))/rate for i in range(len(db)-1) if db[i]<=0<db[i+1]]
 entry={'wave':wave,'target_base_hz':base,'source_hz':1,'depth':depth,'crossings_seconds_after_note':crossings,'db_range':[min(db),max(db)]}
 if depth>=base:
  entry['gate']='diagnostic only: negative target rate, outside this monotonic model'
  return entry
 comparisons=[]
 for observed in crossings:
  if observed<.02:continue
  cycle=round(base*observed+depth*integral(observed,wave))
  lo,hi=0.,2.5
  for _ in range(60):
   mid=(lo+hi)*.5
   if base*mid+depth*integral(mid,wave)<cycle:lo=mid
   else:hi=mid
  predicted=(lo+hi)*.5
  comparisons.append({'cycle':cycle,'observed_seconds':observed,'predicted_seconds':predicted,'error_ms':1000*(observed-predicted)})
 entry['comparisons']=comparisons
 entry['max_absolute_error_ms']=max((abs(x['error_ms']) for x in comparisons),default=0)
 entry['within_1ms']=entry['max_absolute_error_ms']<=1
 entry['interval_mean_frequencies_hz']=[1/(b-a) for a,b in zip(crossings,crossings[1:])]
 return entry
if __name__=='__main__':
 p=argparse.ArgumentParser(description=__doc__);p.add_argument('--directory',type=Path,required=True);p.add_argument('--output',type=Path,required=True);a=p.parse_args()
 report={'scope':'DC volume-LFO cycle clocks, additive Hz; source N target X; monotonic phase gate only','cases':{}}
 for path in sorted(a.directory.glob('w*-f*-d*.wav')):report['cases'][path.stem]=analyze(path)
 a.output.write_text(json.dumps(report,indent=2)+'\n')
 for n,e in report['cases'].items():print(n,e.get('max_absolute_error_ms'),e.get('within_1ms'),e.get('gate',''))
