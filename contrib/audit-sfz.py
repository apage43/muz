#!/usr/bin/env python3
"""Produce reproducible public-root inventory using the engine importer CLI.

The command accepts JSON-lines stdin {path, defines} and emits per-root JSON containing
path, regions, dependencies and opcodes (spelling -> distinct values), or error.
No lexical scan is substituted for runtime preprocessing or inheritance.
"""
import argparse
import hashlib
import json
from pathlib import Path
import subprocess
import struct
import tempfile


def audio_metadata(path):
    """Read WAV/FLAC container metadata without decoding recordings."""
    suffix=path.suffix.lower()
    if suffix not in ('.wav','.flac'): return {}
    with path.open('rb') as f:
        if suffix=='.flac':
            if f.read(4)!=b'fLaC': return dict(audio_error='invalid FLAC header')
            header=f.read(4)
            if len(header)!=4 or header[0]&127!=0: return dict(audio_error='FLAC STREAMINFO absent')
            data=f.read(int.from_bytes(header[1:],'big'))
            if len(data)<34: return dict(audio_error='short FLAC STREAMINFO')
            packed=int.from_bytes(data[10:18],'big')
            return dict(format='flac',sample_rate=packed>>44,channels=((packed>>41)&7)+1,bits_per_sample=((packed>>36)&31)+1,frames=packed&((1<<36)-1))
        header=f.read(12)
        if header[:4]!=b'RIFF' or header[8:]!=b'WAVE': return dict(audio_error='unsupported WAV container')
        result=dict(format='wav');data_bytes=0;align=0
        while chunk:=f.read(8):
            if len(chunk)!=8: break
            kind,size=struct.unpack('<4sI',chunk);start=f.tell()
            if kind==b'fmt ':
                data=f.read(min(size,40))
                if len(data)>=16:
                    encoding,channels,rate,_,align,bits=struct.unpack('<HHIIHH',data[:16])
                    result.update(channels=channels,sample_rate=rate,bits_per_sample=bits,encoding=encoding)
            elif kind==b'data': data_bytes=size
            elif kind==b'smpl':
                data=f.read(size)
                if len(data)>=36:
                    count=struct.unpack_from('<I',data,28)[0]
                    result['loops']=[dict(type=struct.unpack_from('<I',data,36+24*n+4)[0],start=struct.unpack_from('<I',data,36+24*n+8)[0],end_inclusive=struct.unpack_from('<I',data,36+24*n+12)[0]) for n in range(min(count,(len(data)-36)//24))]
            f.seek(start+size+(size&1))
        if align: result['frames']=data_bytes//align
        return result


def main():
    p = argparse.ArgumentParser(description=__doc__)
    p.add_argument('pack', type=Path)
    p.add_argument('--assets', type=Path)
    p.add_argument('--strip-prefix', default='', help='catalog prefix omitted in audit tree')
    p.add_argument('--importer', type=Path, required=True)
    p.add_argument('--output', type=Path, required=True)
    p.add_argument('--prepare', action='store_true', help='also decode samples and prepare runtime resources')
    p.add_argument('--exercise', action='store_true', help='also run native predicate/control/lifecycle smoke; implies preparation')
    a = p.parse_args()
    assets = (a.assets or a.pack/'assets').resolve()
    catalog = json.loads((a.pack/'sfz-catalog.json').read_text())
    roots = [assets/r['path'].removeprefix(a.strip_prefix) for r in catalog['programs']]
    requests=''.join(json.dumps(dict(path=str(root),defines=descriptor.get('defines',{}),source_overlays=descriptor.get('source_overlays',[])))+'\n' for descriptor,root in zip(catalog['programs'],roots))
    command=[str(a.importer.resolve())]+(['--prepare'] if a.prepare else [])+(['--exercise'] if a.exercise else [])
    reports=[]
    with tempfile.TemporaryFile(mode='w+t') as errors:
        process=subprocess.Popen(command,stdin=subprocess.PIPE,stdout=subprocess.PIPE,stderr=errors,text=True)
        process.stdin.write(requests)
        process.stdin.close()
        for line in process.stdout:
            reports.append(json.loads(line))
            if len(reports)%25==0:
                print(f'imported/prepared {len(reports)}/{len(roots)} public roots',flush=True)
        status=process.wait()
        errors.seek(0)
        importer_stderr=errors.read()
        if status:
            raise subprocess.CalledProcessError(status,command,stderr=importer_stderr)

    by_path = {r['path']:r for r in reports}
    output=[]
    identities={}
    for descriptor,root in zip(catalog['programs'], roots):
        report=by_path[str(root)];report['path']=descriptor['path'];report['id']=descriptor['id']
        if 'error' in report: report['error']=report['error'].replace(str(assets),'$assets')
        for diagnostic in report.get('diagnostics',[]):
            if isinstance(diagnostic,dict) and diagnostic.get('source'):
                diagnostic['source']=str(diagnostic['source']).replace(str(assets),'$assets')
        if 'dependencies' in report:
            dependencies=[]
            for name in report['dependencies']:
                path=Path(name)
                try: relative=str(path.relative_to(assets))
                except ValueError: raise ValueError(f'dependency escapes assets: {path}')
                relative=a.strip_prefix+relative
                if relative not in identities:
                    identities[relative]=dict(bytes=path.stat().st_size,sha256=hashlib.file_digest(path.open('rb'),'sha256').hexdigest(),**audio_metadata(path)) if path.is_file() else dict(missing=True)
                dependencies.append(relative)
            report['dependencies']=sorted(dependencies)
            frames=sum(identities[d].get('frames',0) for d in set(dependencies))
            report['estimated_decoded_stereo_frames']=frames
            report['estimated_decoded_stereo_bytes']=frames*8
        output.append(report)
    document=dict(schema=1,catalog_sha256=hashlib.sha256((a.pack/'sfz-catalog.json').read_bytes()).hexdigest(),source=catalog['source'],programs=output,dependencies=identities,importer_stderr=importer_stderr)
    def portable(value):
        if isinstance(value,str): return value.replace(str(assets),'$assets')
        if isinstance(value,list): return [portable(v) for v in value]
        if isinstance(value,dict): return {k:portable(v) for k,v in value.items()}
        return value
    a.output.write_text(json.dumps(portable(document),indent=2,sort_keys=True)+'\n')
    failures=sum('error' in r or any(identities[d].get('missing') for d in r.get('dependencies',[])) for r in output)
    unsupported=sum(bool(r.get('unsupported_behaviors')) for r in output)
    preparation=sum(r.get('prepared') is False for r in output)
    exercise=sum('exercise_error' in r or (a.exercise and r.get('prepared') is True and r.get('exercise',{}).get('passed') is not True) for r in output)
    print(f'{len(output)} public roots audited; {failures} import/dependency failures; {unsupported} roots with unsupported playback; {preparation} preparation failures; {exercise} exercise failures')
    raise SystemExit(bool(failures or unsupported or preparation or exercise))


if __name__=='__main__':main()
