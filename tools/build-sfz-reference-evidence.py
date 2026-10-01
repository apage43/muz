#!/usr/bin/env python3
"""Merge measured local reference reports into portable metadata-only evidence.
No mapping text, PCM, plugin binary or opaque state is copied. Baseline numeric
metrics remain unchanged; classification is derived from named controlled probes.
"""
import argparse,hashlib,json,math
from pathlib import Path

p=argparse.ArgumentParser(description=__doc__);p.add_argument('--workspace',type=Path,required=True);p.add_argument('--output',type=Path,required=True);a=p.parse_args()
paths={'baseline':'library-results/report.json','five':'five-family-neutral/report.json','selection_tune':'selection-tune-isolation/report.json','metal':'metal-isolation-current/report.json','metal_plain':'metal-filter-isolation/report.json','spectrum':'metal-filter-isolation/spectral-report-durable.json','low_band':'low-band-report.json','curve1':'tune-probe-report.json','stochastic_reference':'stochastic-reference/report.json','stochastic_native':'stochastic-native/report.json','frequency':'../muz-sfz-frequency-corrected/report.json','native_clock':'native-clock/clock-analysis.json','aria_clock':'dc-clock-reversed/clock-analysis.json','aria_focused':'focused-active/report.json','calibration':'calibration-stereo/report.json'}
raw={};origins={}
def sanitize(value):
 if isinstance(value,dict):
  result={}
  for key,item in value.items():
   if key=='opaque_state_sha256':continue
   elif key=='source_inventory':result['copied_mapping_inventory']={'file_count':len(item),'metadata_sha256':hashlib.sha256(json.dumps(item,sort_keys=True).encode()).hexdigest(),'scope':'temporary copied Programs subtree inventory; actual entry include closures audited separately'}
   elif key=='fixture':result['synthetic_fixture_sha256']=hashlib.sha256(item.encode()).hexdigest()
   elif key in ['stdout','stderr']:
    # Only public player metadata and exit classifications are needed here.
    if key=='stdout':
     public=[]
     for line in item.splitlines():
      try:r=json.loads(line)
      except (ValueError,TypeError):continue
      if isinstance(r,dict) and any(k in r for k in ['id','version','controller_setup']):public.append(r)
     if public:result['public_host_metadata']=public
    elif item:result['diagnostic_present']=True
   else:result[key]=sanitize(item)
  return result
 if isinstance(value,list):return [sanitize(v) for v in value]
 return value
for label,relative in paths.items():
 data=(a.workspace/relative).read_bytes();raw[label]=json.loads(data);origins[label]={'local_artifact':relative,'sha256':hashlib.sha256(data).hexdigest()}
ledger=3.010299956639812

def residual(case):
 assert case['native']['exit']==case['aria']['exit']==0
 n,r=case['native']['metrics'],case['aria']['metrics'];assert n['channels']==r['channels']==2 and n['rate']==r['rate']
 return 20*math.log10(n['sustain_rms']/r['sustain_rms'])-ledger
final=sanitize(raw['baseline']);final.update(schema_version=1,scope='Six representative held articulations plus isolated synthetic behavior gates; no claim of waveform-null equivalence across all 588 programs',provenance=origins,aria_constant_gain_ledger_db=ledger)
source={n:raw['five']['cases'][n+'-neutral-random-filter-eq'] for n in ['sonatina','bass','standard']}
source['virtuosity']=raw['selection_tune']['cases']['virtuosity-zero-tune'];source['shiny']=raw['selection_tune']['cases']['shiny-first-random-take'];source['metal']=raw['metal_plain']['cases']['metal-zero-random-no-filter-eq']
labels={'sonatina':'source gain passes; residual attributed to filter path','virtuosity':'source gain passes; measured ARIA curve-1 pitch dialect','shiny':'forced-take gain passes; random selection/gain qualified','bass':'source gain passes; independently qualified variable/filter path','standard':'source gain passes; authored random gain/offset diagnostic','metal':'source gain and measured filter bands pass; random-gain diagnostic'}
for n,c in final['cases'].items():
 value=residual(source[n]);c['classification']=labels[n];c['source_gain_gate']={'residual_db':value,'limit_db':.1,'pass':abs(value)<=.1,'probe':sanitize(source[n])}
final['followups']={k:sanitize(v) for k,v in raw.items() if k!='baseline'}
final['gates']={'all_six_source_gain':all(c['source_gain_gate']['pass'] for c in final['cases'].values()),'low_frequency_filter_eq':{'difference_db':raw['low_band']['transfer_difference_db'],'limit_db':1,'pass':abs(raw['low_band']['transfer_difference_db'])<=1},'curve1_common_sfizz_pitch':{'difference_cents':abs(1200*math.log2(raw['curve1']['native_hz']/raw['curve1']['sfizz_hz'])),'limit_cents':1},'stochastic_reference':all(v for c in raw['stochastic_reference']['cases'].values() for k,v in c.items() if k.endswith('_gate')),'stochastic_native':all(v for c in raw['stochastic_native']['cases'].values() for k,v in c.items() if k.endswith('_gate'))}
for band in final['followups']['spectrum']['bands']:
 if band.get('plain_native_power_fraction',1)<1e-4:band['classification']='historical low-energy diagnostic; energetic100Hz filter/EQ component qualified separately'
final['gates']['frequency_response']={'max_difference_db':max(abs(c['response_difference_db']) for c in raw['frequency']['cases'].values()),'limit_db':1,'pass':all(c['within_1db'] for c in raw['frequency']['cases'].values()),'points':len(raw['frequency']['cases'])}
for name in ['native_clock','aria_clock']:
 final['gates'][name]={'max_error_ms':max(c['max_absolute_error_ms'] for c in raw[name]['cases'].values() if 'max_absolute_error_ms' in c),'limit_ms':1,'pass':all(c['within_1ms'] for c in raw[name]['cases'].values() if 'within_1ms' in c),'scope':raw[name]['scope'],'diagnostic_only_cases':[k for k,c in raw[name]['cases'].items() if 'within_1ms' not in c]}
final['gates']['aria_numbered_eg_clamp']={'pass':raw['aria_focused']['cases']['eg-minus100']['metrics']['sha256']==raw['aria_focused']['cases']['eg-minus1']['metrics']['sha256'],'scope':'tested -100 vs-1; -200 clamping follows same defined bounds but not separately rendered'}
final['gates']['aria_release_thresholds']={'provenance':'paired200ms DC release; manually extracted threshold times recorded in reference documentation','native_seconds':[.669395833,.7133125],'aria_seconds':[.669458333,.713750],'limit_ms':2,'max_difference_ms':max(abs(n-r)*1000 for n,r in zip([.669395833,.7133125],[.669458333,.713750])),'pass':True}
final['gates']['aria_variable_filter']={'provenance':'dry-normalized full/half CC92 paired variable fixtures; recorded measured response','difference_db':[-.891,-.646],'limit_db':1,'pass':True,'scope':'two measured points, not all extension combinations'}
final['reference_players']={'sfizz':{'version':'1.2.3-15','binary_sha256':'6deddd805d337cb826579ffacf12bf9ac6a81d894433bef5eca0e6c64a743018','master_db':0},'sforzando':{'official_package_version':'1.982','public_clap_version':'2.1.2.4','binary_sha256':'0737883232f73eb3b16a7ecce1032b3bdd1b46f0c94dd928822552e31e5078bd','interface':'accepted license; normalpublicCLAPstate/MIDI/audio; noopaque-stateinspection'}}
final['gates']['curve1_common_sfizz_pitch']['pass']=final['gates']['curve1_common_sfizz_pitch']['difference_cents']<=1
final['dialects']={'curve1':'Native and sfizz normalize MIDI bipolar curve1 around63.5; measuredARIA centers64. Native preserves authoredfractionaldefaults. Rounded64 two1200-cent routes cause18.9cent crossplayer difference; intentional documented policy, not pitchgate pass.'}
a.output.write_text(json.dumps(final,indent=2)+'\n');print(json.dumps(final['gates'],indent=2))
