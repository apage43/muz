#!/usr/bin/env python3
"""Public-player stochastic probe: one continuous instance per1000-note case.
Run inside the same isolated environment as sfz-reference-clap.py. The explicit
fixture must be the file reopened by the caller's opaque public state. Requires
numpy; never decodes player state. Produces synthetic WAV/report data outside Git.
"""
import argparse,json,math,struct,subprocess,sys,wave
from pathlib import Path
import numpy as np

p=argparse.ArgumentParser(description=__doc__)
for name in ['host','plugin','state','fixture','output']:p.add_argument('--'+name,type=Path,required=True)
p.add_argument('--notes',type=int,default=1000)
p.add_argument('--muz',type=Path,help='optional native CLI for the same isolated synthetic cases')
a=p.parse_args();a.output.mkdir(parents=True,exist_ok=True);rate=48000
assert a.notes>=100
sample=a.fixture.parent/'stochastic-dc.wav'
with wave.open(str(sample),'wb') as w:
 w.setparams((1,2,rate,0,'NONE','not compressed'));w.writeframes(struct.pack('<h',4000)*rate)
events=[]
for i in range(a.notes):
 events.extend([{'frame':i*960,'data':[0x90,60,100]},{'frame':i*960+720,'data':[0x80,60,64]}])
eventfile=a.output/'events.json';eventfile.write_text(json.dumps(events))
cases={'zero':'amp_random=0','positive':'amp_random=3','negative':'amp_random=-3',
       'selection':'<region> sample=stochastic-dc.wav lorand=0 hirand=0.5 pan=-100\n<region> sample=stochastic-dc.wav lorand=0.5 hirand=1 pan=100'}
report={'notes_per_instance':a.notes,'schedule':'one note per20ms,off15ms; measure5–10ms',
        'reference_interface':'public CLAP MIDI/events/render; opaque state unchanged','cases':{}}
levels={}
for name,fields in cases.items():
 base='<control> set_cc7=127 set_cc11=127\n<group> key=60 amp_veltrack=0 loop_mode=loop_continuous ampeg_attack=0 ampeg_release=0.001\n'
 text=base+(fields if name=='selection' else '<region> sample=stochastic-dc.wav '+fields)
 a.fixture.write_text(text);output=a.output/(name+'.wav')
 cmd=[sys.executable,str(a.host),'--plugin',str(a.plugin),'--sfz',str(a.fixture),'--state',str(a.state),'--output',str(output),'--seconds',str(a.notes*.02),'--stereo','--authored-cc-defaults','--midi-events',str(eventfile)]
 r=subprocess.run(cmd,capture_output=True,text=True,timeout=120);record={'fixture':text,'exit':r.returncode,'stdout':r.stdout,'stderr':r.stderr};report['cases'][name]=record
 if r.returncode!=0:continue
 with wave.open(str(output)) as w:
  assert w.getframerate()==rate and w.getnchannels()==2 and w.getsampwidth()==2
  x=np.frombuffer(w.readframes(w.getnframes()),dtype='<i2').astype(float).reshape(-1,2)/32768
 values=np.array([np.sqrt(np.mean(x[i*960+240:i*960+480]**2,axis=0)) for i in range(a.notes)])
 levels[name]=np.sqrt(np.mean(values**2,axis=1))
 if name=='selection':
  left=int(np.count_nonzero(values[:,0]>values[:,1]));bound=4*math.sqrt(a.notes*.25)
  record.update(left_count=left,right_count=a.notes-left,binomial_four_sigma_bound=bound,selection_gate=abs(left-a.notes*.5)<=bound)
for name,sign in [('positive',1),('negative',-1)]:
 if name not in levels or 'zero' not in levels:continue
 db=20*np.log10(levels[name]/float(np.mean(levels['zero'])));mean=float(db.mean());variance=float(db.var());meanbound=4*math.sqrt(.75/a.notes);varbound=4*.75*math.sqrt(.8/a.notes)
 record=report['cases'][name];record.update(min_db=float(db.min()),max_db=float(db.max()),mean_db=mean,variance_db2=variance,mean_expected_db=sign*1.5,variance_expected_db2=.75,mean_four_sigma_bound_db=meanbound,variance_four_sigma_bound_db2=varbound,
  range_gate=bool(np.all(db>=min(0,sign*3)-.005) and np.all(db<=max(0,sign*3)+.005)),mean_gate=abs(mean-sign*1.5)<=meanbound,variance_gate=abs(variance-.75)<=varbound)
if a.muz:
 native={};native_levels={}
 for name,case in report['cases'].items():
  a.fixture.write_text(case['fixture']);source=a.output/(name+'.muz');output=a.output/(name+'-native.wav')
  notes=','.join('note(60,0.03b,at='+str(round(i*.04,6))+'b,velocity=100/127).gate(1)' for i in range(a.notes))
  source.write_text('song({tempo:120,tail:0,tracks:[track("ref",stack([control(7,127),control(11,127),control(10,64),'+notes+']),sfz('+json.dumps(str(a.fixture))+',{seed:7,max_voices:16}),{gain:0})]})')
  r=subprocess.run([str(a.muz),'render',str(source),'-o',str(output),'--sample-rate','48000','--seconds',str(a.notes*.02),'--tail','0','--format','pcm16'],capture_output=True,text=True,timeout=120)
  record={'exit':r.returncode,'stderr':r.stderr};native[name]=record
  if r.returncode:continue
  with wave.open(str(output)) as w:
   x=np.frombuffer(w.readframes(w.getnframes()),dtype='<i2').astype(float).reshape(-1,2)/32768
  values=np.array([np.sqrt(np.mean(x[i*960+240:i*960+480]**2,axis=0)) for i in range(a.notes)])
  native_levels[name]=np.sqrt(np.mean(values**2,axis=1))
  if name=='selection':
   left=int(np.count_nonzero(values[:,0]>values[:,1]));record.update(left_count=left,right_count=a.notes-left,selection_gate=abs(left-a.notes*.5)<=4*math.sqrt(a.notes*.25))
 for name,sign in [('positive',1),('negative',-1)]:
  if name not in native_levels or 'zero' not in native_levels:continue
  db=20*np.log10(native_levels[name]/native_levels['zero'].mean());mean=float(db.mean());variance=float(db.var())
  native[name].update(min_db=float(db.min()),max_db=float(db.max()),mean_db=mean,variance_db2=variance,mean_gate=abs(mean-sign*1.5)<=4*math.sqrt(.75/a.notes),variance_gate=abs(variance-.75)<=4*.75*math.sqrt(.8/a.notes),range_gate=bool(np.all(db>=min(0,sign*3)-.005) and np.all(db<=max(0,sign*3)+.005)))
 report['native']={'seed':7,'cases':native}
(a.output/'report.json').write_text(json.dumps(report,indent=2)+'\n');print(json.dumps(report,indent=2))
