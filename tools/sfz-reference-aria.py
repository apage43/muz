#!/usr/bin/env python3
"""Replay public CLAP state pointing at our mutable synthetic SFZ, without decoding state.
Run inside an approved narrow reference sandbox. First import --fixture through
the player's GUI and save opaque state with sfz-reference-clap.py --save-state.
Each case replaces only that synthetic fixture; all program interfaces are public.
"""
import argparse,hashlib,importlib.util,json,subprocess,sys
from pathlib import Path
p=argparse.ArgumentParser(description=__doc__)
p.add_argument('--host',type=Path,required=True)
p.add_argument('--plugin',type=Path,required=True)
p.add_argument('--state',type=Path,required=True)
p.add_argument('--fixture',type=Path,required=True)
p.add_argument('--cases',type=Path,required=True,help='JSON mapping name to region opcodes, or {control,region}')
p.add_argument('--output',type=Path,required=True)
p.add_argument('--seconds',type=float,default=1)
p.add_argument('--note-off',type=float,default=0.625)
a=p.parse_args();a.output.mkdir(parents=True,exist_ok=True)
spec=importlib.util.spec_from_file_location('reference',Path(__file__).with_name('sfz-reference.py'))
r=importlib.util.module_from_spec(spec);spec.loader.exec_module(r)
report={'plugin_sha256':hashlib.sha256(a.plugin.read_bytes()).hexdigest(),'opaque_state_sha256':hashlib.sha256(a.state.read_bytes()).hexdigest(),'scope':'synthetic public CLAP state replay; no proprietary state decoding','cases':{}}
for name,case in json.loads(a.cases.read_text()).items():
 fields={'sample':'tone.wav','key':'60','ampeg_release':'0.1'}
 region=case['region'] if isinstance(case,dict) else case
 control=case.get('control','') if isinstance(case,dict) else ''
 for token in region.split():
  key,sep,value=token.partition('=')
  if sep:fields[key]=value
 a.fixture.write_text('<control> set_cc7=127 set_cc11=127 set_cc10=64 set_cc117=127 '+control+'\n<region> '+' '.join(k+'='+v for k,v in fields.items())+'\n')
 output=a.output/(name+'.wav')
 proc=subprocess.run([sys.executable,str(a.host),'--plugin',str(a.plugin),'--sfz',str(a.fixture),'--state',str(a.state),'--output',str(output),'--seconds',str(a.seconds),'--note-off',str(a.note_off)],capture_output=True,text=True)
 entry={'exit':proc.returncode,'stdout':proc.stdout,'stderr':proc.stderr,'fixture':a.fixture.read_text()}
 if proc.returncode==0:entry.update(metrics=r.metrics(output),features=r.feature_metrics(output),pitch_trace=r.pitch_trace(output))
 report['cases'][name]=entry
 print(name,proc.returncode,flush=True)
(a.output/'report.json').write_text(json.dumps(report,indent=2)+'\n')
