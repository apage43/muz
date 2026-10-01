#!/usr/bin/env python3
"""Dry frequency-response comparison using explicit existing sfizz/muz tools."""
import argparse
import hashlib
import importlib.util
import json
import math
from pathlib import Path
import subprocess

spec = importlib.util.spec_from_file_location('sfz_reference', Path(__file__).with_name('sfz-reference.py'))
reference = importlib.util.module_from_spec(spec)
spec.loader.exec_module(reference)

FILTERS = {
    'dry': '',
    'lowpass-2p': 'fil_type=lpf_2p cutoff=1000 resonance=0',
    'highpass-1p': 'fil_type=hpf_1p cutoff=1000 resonance=0',
    'two-filters': 'fil_type=lpf_2p cutoff=1000 resonance=0 fil2_type=hpf_1p cutoff2=250 resonance2=0',
    'eq-peak': 'eq1_freq=1000 eq1_bw=1 eq1_gaincc48=6',
}


def run(root, library, muz):
    root.mkdir(parents=True, exist_ok=True)
    report = {'library': library, 'library_sha256': hashlib.sha256(Path(library).read_bytes()).hexdigest(),
              'muz': muz, 'muz_sha256': hashlib.sha256(Path(muz).read_bytes()).hexdigest(),
              'settings': {'rate':48000,'reference_block':48,'reference_host_gain_db':0,'cc7':127,'cc11':127},
              'scope':'six steady frequencies; dry static filters and one EQ band; no dynamic/resonance/corpus claim',
              'cases': {}}
    for hz in (100,250,500,1000,2000,4000):
        sample = root/f'tone{hz}.wav'
        reference.sample(sample, hz)
        for name, opcodes in FILTERS.items():
            key = f'{name}-{hz}'
            sfz, api, output, source = [root/(key+ext) for ext in ('.sfz','-api.wav','-muz.wav','.muz')]
            sfz.write_text('<control> set_cc7=127 set_cc11=127 set_cc48=127\n<region> key=60 sample='+sample.name
                           +' amp_veltrack=0 ampeg_release=0.1 '+opcodes+'\n')
            unknown = reference.unknown(library, sfz)
            reference.api_render(library, sfz, api)
            source.write_text('song({tempo:120,tail:0,tracks:[track("response",'
                              'note(60,1b,at=0.25b,velocity=100/127).gate(1),sfz('
                              +json.dumps(str(sfz))+'),{gain:0})]})\n')
            proc = subprocess.run([muz,'render',str(source),'-o',str(output),'--sample-rate','48000',
                                   '--seconds','1','--tail','0','--format','pcm16'],capture_output=True,text=True)
            entry = {'frequency_hz':hz,'filter':name,'reference':unknown,
                     'reference_metrics':reference.metrics(api),
                     'muz':{'exit':proc.returncode,'stderr':proc.stderr}}
            if proc.returncode == 0:
                entry['muz']['metrics'] = reference.metrics(output)
            report['cases'][key] = entry
    for entry in report['cases'].values():
        hz = entry['frequency_hz']
        dry = report['cases'][f'dry-{hz}']
        rr = entry['reference_metrics']['sustain_rms']/dry['reference_metrics']['sustain_rms']
        entry['reference_response_db'] = 20*math.log10(rr) if rr else None
        if entry['muz']['exit'] == 0 and dry['muz']['exit'] == 0:
            nr = entry['muz']['metrics']['sustain_rms']/dry['muz']['metrics']['sustain_rms']
            entry['muz_response_db'] = 20*math.log10(nr) if nr else None
            entry['response_difference_db'] = entry['muz_response_db']-entry['reference_response_db'] if nr and rr else None
            entry['within_1db'] = abs(entry['response_difference_db']) <= 1 if entry['response_difference_db'] is not None else False
    (root/'report.json').write_text(json.dumps(report,indent=2)+'\n')
    return report


if __name__ == '__main__':
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--library',required=True)
    parser.add_argument('--muz',required=True)
    parser.add_argument('--output',type=Path,required=True)
    args = parser.parse_args()
    report = run(args.output.resolve(),args.library,str(Path(args.muz).resolve()))
    for name, entry in report['cases'].items():
        print(name,entry['muz']['exit'],entry.get('response_difference_db'),entry.get('within_1db'))
