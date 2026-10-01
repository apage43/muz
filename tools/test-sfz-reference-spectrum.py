#!/usr/bin/env python3
"""Check paired-transfer cancellation and invalid measurement handling."""
import importlib.util,tempfile,unittest,wave
from pathlib import Path
import numpy as np
spec=importlib.util.spec_from_file_location('spectrum',Path(__file__).with_name('sfz-reference-spectrum.py'))
s=importlib.util.module_from_spec(spec);spec.loader.exec_module(s)

class SpectrumTests(unittest.TestCase):
 def fixture(self,path,gain,rate=48000):
  x=(np.sin(np.arange(rate)*2*np.pi*100/rate)*6000*gain).astype('<i2')
  with wave.open(str(path),'wb') as w:
   w.setparams((2,2,rate,0,'NONE','not compressed'));w.writeframes(np.repeat(x[:,None],2,axis=1).tobytes())
 def test_independent_master_gain_cancels_in_transfer(self):
  with tempfile.TemporaryDirectory() as tmp:
   paths=[Path(tmp)/str(i) for i in range(4)]
   for p,g in zip(paths,[.5,.25,1,.5]):self.fixture(p,g)
   r=s.compare(paths);low=r['bands'][0]
   self.assertLess(abs(low['difference_db']),.002)
   self.assertAlmostEqual(low['native_transfer_db'],-6.0206,places=2)
 def test_rate_mismatch_rejected(self):
  with tempfile.TemporaryDirectory() as tmp:
   paths=[Path(tmp)/str(i) for i in range(4)]
   for i,p in enumerate(paths):self.fixture(p,1,44100 if i==0 else 48000)
   with self.assertRaisesRegex(ValueError,'equal rates'):s.compare(paths)
 def test_insufficient_window_rejected(self):
  with tempfile.TemporaryDirectory() as tmp:
   p=Path(tmp)/'tone.wav';self.fixture(p,1)
   with self.assertRaisesRegex(ValueError,'shorter'):s.compare([p]*4,start=.25,end=.251)

if __name__=='__main__':unittest.main()
