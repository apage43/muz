#!/usr/bin/env python3
"""Installed sfizz voice traces for CC range entry, in-range change, and repeated values."""
import argparse,ctypes as C,importlib.util,json,hashlib,math,struct,wave
from pathlib import Path
sp=importlib.util.spec_from_file_location('reference',Path(__file__).with_name('sfz-reference.py'))
r=importlib.util.module_from_spec(sp);sp.loader.exec_module(r)
p=argparse.ArgumentParser(description=__doc__);p.add_argument('--library',required=True);p.add_argument('--output',type=Path,required=True);a=p.parse_args();a.output.mkdir(parents=True,exist_ok=True)
r.sample(a.output/'tone.wav');l=C.CDLL(a.library);P=C.c_void_p;l.sfizz_create_synth.restype=P
for name,args,ret in [('sfizz_free',[P],None),('sfizz_load_file',[P,C.c_char_p],C.c_bool),('sfizz_set_samples_per_block',[P,C.c_int],None),('sfizz_send_cc',[P,C.c_int,C.c_int,C.c_int],None),('sfizz_send_note_on',[P,C.c_int,C.c_int,C.c_int],None),('sfizz_set_volume',[P,C.c_float],None),('sfizz_set_sample_rate',[P,C.c_float],None),('sfizz_get_num_active_voices',[P],C.c_int),('sfizz_render_block',[P,C.POINTER(C.POINTER(C.c_float)),C.c_int,C.c_int],None)]:
 f=getattr(l,name);f.argtypes=args;f.restype=ret
report={}
sfz=a.output/'cc-trigger.sfz';sfz.write_text('<region> sample=tone.wav on_locc1=50 on_hicc1=100 loop_mode=loop_continuous loop_start=0 loop_end=47999\n')
synth=l.sfizz_create_synth()
try:
 l.sfizz_set_samples_per_block(synth,64);assert l.sfizz_load_file(synth,str(sfz).encode())
 buffers=[(C.c_float*64)() for _ in range(2)];channels=(C.POINTER(C.c_float)*2)(*buffers);trace=[]
 for value in [0,64,70,70,0,70]:
  l.sfizz_send_cc(synth,0,1,value)
  l.sfizz_render_block(synth,channels,2,64)
  trace.append({'cc':value,'voices':l.sfizz_get_num_active_voices(synth)})
 report={'reference_library_sha256':hashlib.sha256(Path(a.library).read_bytes()).hexdigest(),'fixture':sfz.read_text(),'trace':trace,'opcode_audit':r.unknown(a.library,sfz),'matches_sfizz_123_trace':[step['voices'] for step in trace]==[0,1,2,2,2,3]}
finally:l.sfizz_free(synth)

# Distinct DC samples and stereo sides identify sequence position without decoding
# internal player state. Query active voice counts through the public sfizz API.
for filename,value in [('dc-one.wav',8000),('dc-two.wav',4000)]:
 with wave.open(str(a.output/filename),'wb') as wav:
  wav.setparams((1,2,48000,0,'NONE','not compressed'))
  wav.writeframes(struct.pack('<h',value)*48000)
sequence_sf=a.output/'cc-sequence.sfz'
sequence_sf.write_text('<control> set_cc7=127 set_cc11=127\n'
 '<group> on_locc1=50 on_hicc1=100 seq_length=2 amp_veltrack=0 loop_mode=loop_continuous loop_start=0 loop_end=47999\n'
 '<region> sample=dc-one.wav seq_position=1 pan=-100\n'
 '<region> sample=dc-two.wav seq_position=2 pan=100\n')
synth=l.sfizz_create_synth()
try:
 l.sfizz_set_samples_per_block(synth,64);l.sfizz_set_sample_rate(synth,48000);l.sfizz_set_volume(synth,0)
 assert l.sfizz_load_file(synth,str(sequence_sf).encode())
 buffers=[(C.c_float*64)() for _ in range(2)];channels=(C.POINTER(C.c_float)*2)(*buffers)
 sequence=[]
 for value in [64,64,70]:
  l.sfizz_send_cc(synth,0,1,value);l.sfizz_render_block(synth,channels,2,64)
  sequence.append({'cc':value,'voices':l.sfizz_get_num_active_voices(synth),
                   'left_last':float(buffers[0][-1]),'right_last':float(buffers[1][-1])})
 report['sequence']={'fixture':sequence_sf.read_text(),'trace':sequence,
                     'expected_started_positions':[1,None,1]}
finally:l.sfizz_free(synth)

# Separate synth instances avoid voice overlap. CC vs note amplitude probes use
# a DC sample; pitch probes use the same 440-Hz wave with explicit root 60.
cases={
 'cc64-velocity':('sample=dc-one.wav amp_veltrack=100 on_locc1=1 on_hicc1=127', 'cc',64),
 'cc100-velocity':('sample=dc-one.wav amp_veltrack=100 on_locc1=1 on_hicc1=127','cc',100),
 'note64-velocity':('sample=dc-one.wav key=60 amp_veltrack=100','note',64),
 'note100-velocity':('sample=dc-one.wav key=60 amp_veltrack=100','note',100),
 'cc-pitch-default':('sample=tone.wav on_locc1=1 on_hicc1=127','cc',100),
 'cc-pitch-key64':('sample=tone.wav key=64 on_locc1=1 on_hicc1=127','cc',100),
 'cc-pitch-key64-center60':('sample=tone.wav key=64 pitch_keycenter=60 on_locc1=1 on_hicc1=127','cc',100),
 'note-pitch60':('sample=tone.wav key=60','note',100),
}
report['audio']={}
for name,(region,event,value) in cases.items():
 sfz=a.output/(name+'.sfz');sfz.write_text('<control> set_cc7=127 set_cc11=127\n<region> '+region+'\n')
 synth=l.sfizz_create_synth()
 try:
  l.sfizz_set_samples_per_block(synth,64);l.sfizz_set_sample_rate(synth,48000);l.sfizz_set_volume(synth,0)
  assert l.sfizz_load_file(synth,str(sfz).encode())
  buffers=[(C.c_float*64)() for _ in range(2)];channels=(C.POINTER(C.c_float)*2)(*buffers);pcm=[]
  if event=='cc':l.sfizz_send_cc(synth,0,1,value)
  else:l.sfizz_send_note_on(synth,0,60,value)
  for frame in range(0,48000,64):
   l.sfizz_render_block(synth,channels,2,64)
   pcm.extend(sample for index in range(64) for sample in (float(buffers[0][index]),float(buffers[1][index])))
  wavpath=a.output/(name+'.wav')
  with wave.open(str(wavpath),'wb') as wav:
   wav.setparams((2,2,48000,0,'NONE','not compressed'))
   wav.writeframes(b''.join(struct.pack('<h',max(-32768,min(32767,round(value*32768)))) for value in pcm))
  report['audio'][name]={'fixture':sfz.read_text(),'event':event,'value':value,
                         'metrics':r.metrics(wavpath),'features':r.feature_metrics(wavpath)}
 finally:l.sfizz_free(synth)
report['cc64_to_100_amplitude_ratio']=report['audio']['cc64-velocity']['metrics']['sustain_rms']/report['audio']['cc100-velocity']['metrics']['sustain_rms']
report['note64_to_100_amplitude_ratio']=report['audio']['note64-velocity']['metrics']['sustain_rms']/report['audio']['note100-velocity']['metrics']['sustain_rms']
(a.output/'report.json').write_text(json.dumps(report,indent=2)+'\n');print(json.dumps(report,indent=2))
