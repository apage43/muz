#!/usr/bin/env python3
"""Export committed, source-pinned calibration measurements to native zone gains."""
import argparse
import json
import math
from pathlib import Path

PACK = Path(__file__).resolve().parent


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--check', action='store_true')
    args = parser.parse_args()
    data = json.loads((PACK / 'calibration.json').read_text())
    pins = {r['path']: r['sha256'] for r in json.loads((PACK / 'manifest.json').read_text())['files']}
    groups = {}
    for record in data['files']:
        assert pins[record['source'].removeprefix('assets/')] == record['source_sha256'], record['source']
        gain = record['constant_gain_db']
        assert math.isfinite(gain) and -120 <= gain <= 120
        assert abs(gain - (data['target_dbfs'] - record['source_body_rms_dbfs'])) < 1e-9
        group = groups.setdefault(record['instrument'], {})
        assert record['index'] not in group
        group[record['index']] = gain
    body = '// Generated from calibration.json by export-calibration.py; original recording gains.\n'
    for name, values in groups.items():
        assert sorted(values) == list(range(len(values))), name
        body += f'let {name} = ' + json.dumps([values[i] for i in range(len(values))]) + ';\n'
    path = PACK / 'calibration-gains.muz'
    if args.check:
        # Formatting is allowed to change whitespace, not the measured values.
        import re
        assert re.sub(r'\s+', '', path.read_text()) == re.sub(r'\s+', '', body), 'regenerate calibration-gains.muz'
    else:
        path.write_text(body)
    print(f'{len(data["files"])} source-pinned zone gains verified')


if __name__ == '__main__':
    main()
