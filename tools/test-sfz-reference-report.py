#!/usr/bin/env python3
"""Regression tests for evidence-table/maxima reconciliation and invalid comparisons."""
import importlib.util
import math
import unittest
from pathlib import Path
spec=importlib.util.spec_from_file_location('report',Path(__file__).with_name('sfz-reference-report.py'))
module=importlib.util.module_from_spec(spec);spec.loader.exec_module(module)

def evidence():
    return {'cases':{name:{'entry':'program.sfz','midi':{'key':60},
                          'native':{'exit':0,'metrics':{'channels':2,'rate':48000,'sustain_rms':1}},
                          'aria':{'exit':0,'metrics':{'channels':2,'rate':48000,'sustain_rms':1}}}
                     for name in module.LABELS}}

class EvidenceTests(unittest.TestCase):
    def test_maximum_is_derived_from_same_records_as_rows(self):
        source=evidence();source['cases']['metal']['native']['metrics']['sustain_rms']=10**(2/20)
        text=module.render(source,0)
        self.assertIn('| METAL-GTX | `program.sfz` | 60 | +2.0000 |',text)
        self.assertIn('largest valid absolute residual is 2.0000 dB (METAL-GTX)',text)
        self.assertIn('whole-patch diagnostic; selection/component attribution open',text)

    def test_unequal_channels_are_not_in_numeric_maximum(self):
        source=evidence();source['cases']['metal']['aria']['metrics']['channels']=1
        source['cases']['metal']['native']['metrics']['sustain_rms']=100
        text=module.render(source,0)
        self.assertIn('unequal channel aggregation; comparison invalid',text)
        self.assertIn('largest valid absolute residual is 0.0000 dB',text)
        self.assertIn('Incomplete or invalid comparisons: METAL-GTX.',text)

    def test_failed_or_missing_measurements_remain_explicit(self):
        source=evidence();source['cases']['standard']['native']['exit']=1
        del source['cases']['metal']
        text=module.render(source,0)
        self.assertIn('execution failed or missing',text)
        self.assertIn('missing case',text)
        self.assertIn('Incomplete or invalid comparisons: Standard Guitar, METAL-GTX.',text)

if __name__=='__main__':unittest.main()
