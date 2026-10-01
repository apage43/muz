#!/usr/bin/env python3
"""Installed sfizz voice traces for omitted vs explicit SFZ sequence length."""
import argparse,ctypes as C,importlib.util,json
from pathlib import Path
sp=importlib.util.spec_from_file_location('reference',Path(__file__).with_name('sfz-reference.py'))
r=importlib.util.module_from_spec(sp);sp.loader.exec_module(r)
p=argparse.ArgumentParser(description=__doc__);p.add_argument('--library',required=True);p.add_argument('--output',type=Path,required=True);a=p.parse_args();a.output.mkdir(parents=True,exist_ok=True)
r.sample(a.output/'tone.wav');l=C.CDLL(a.library);P=C.c_void_p;l.sfizz_create_synth.restype=P
for name,args,ret in [('sfizz_free',[P],None),('sfizz_load_file',[P,C.c_char_p],C.c_bool),('sfizz_set_samples_per_block',[P,C.c_int],None),('sfizz_send_note_on',[P,C.c_int,C.c_int,C.c_int],None),('sfizz_get_num_active_voices',[P],C.c_int),('sfizz_render_block',[P,C.POINTER(C.POINTER(C.c_float)),C.c_int,C.c_int],None)]:
 f=getattr(l,name);f.argtypes=args;f.restype=ret
report={}
for length in [None,3,4]:
 for position in [1,2,3,4,5,6,7,8]:
  name=f'length{length}-position{position}';sfz=a.output/(name+'.sfz');sfz.write_text('<region> sample=tone.wav key=60 seq_position='+str(position)+(' seq_length='+str(length) if length else '')+'\n')
  synth=l.sfizz_create_synth()
  try:
   l.sfizz_set_samples_per_block(synth,64);assert l.sfizz_load_file(synth,str(sfz).encode())
   buffers=[(C.c_float*64)() for _ in range(2)];channels=(C.POINTER(C.c_float)*2)(*buffers);trace=[]
   for hit in range(4):
    l.sfizz_send_note_on(synth,0,60,100)
    for _ in range(50):l.sfizz_render_block(synth,channels,2,64)
    trace.append(l.sfizz_get_num_active_voices(synth))
   report[name]={'length':length,'position':position,'voice_counts':trace}
  finally:l.sfizz_free(synth)
(a.output/'report.json').write_text(json.dumps(report,indent=2)+'\n');print(json.dumps(report,indent=2))
