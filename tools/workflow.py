"""Manual integration check for transport/reload/job lifecycle, using disposable synthetic music."""
import argparse, json, socket, subprocess, tempfile, time
from pathlib import Path

args=argparse.ArgumentParser()
args.add_argument('--audio',action='store_true')
args.add_argument('--piano',action='store_true')
args.add_argument('--plugin')
options=args.parse_args()
binary=Path('target/release/muz').resolve()
with tempfile.TemporaryDirectory(prefix='muz-workflow-') as tmp:
    root=Path(tmp);source=root/'case.muz';sock=root/'control.sock'
    instrument=('plugin('+json.dumps(options.plugin)+')') if options.plugin else ('piano()' if options.piano else 'synth("bell")')
    text='song({sections:[section("a",4bars)],tracks:[track("test",phrase("C4:q E4:q G4:q B4:q").repeat(4),'+instrument+',{gain:-24})],tail:0.2})'
    if options.plugin and options.plugin.endswith('.clap'):
        text=text.replace('.repeat(4)', '.repeat(4).express({tuning:[[0,0],[1,0.1]],brightness:[[0,0.3],[1,0.6]]})')
    source.write_text(text)
    log=(root/'server.log').open('w')
    cmd=[str(binary),'serve',str(source),'--socket',str(sock),'--stopped']
    if not options.audio:cmd+=['--headless']
    process=subprocess.Popen(cmd,stdout=log,stderr=log)
    def call(command,**fields):
        with socket.socket(socket.AF_UNIX) as client:
            client.settimeout(10);client.connect(str(sock));client.sendall((json.dumps(dict(command=command,**fields))+'\n').encode())
            data=b''
            while b'\n' not in data:data+=client.recv(65536)
            response=json.loads(data);assert response['ok'],response
            return response['result']
    def until(predicate,seconds=20):
        deadline=time.monotonic()+seconds
        while time.monotonic()<deadline:
            if process.poll() is not None:raise RuntimeError((root/'server.log').read_text())
            try:
                value=predicate()
                if value:return value
            except (FileNotFoundError,ConnectionRefusedError):pass
            time.sleep(.1)
        raise TimeoutError((root/'server.log').read_text())
    try:
        status=until(lambda:call('status'))
        tokens=[d['instance_token'] for d in status['devices']]
        call('loop',section='a');call('seek',beat=2);call('play')
        until(lambda:call('status')['callback_count']>10)
        source.write_text('song({broken')
        until(lambda:call('status')['last_error'])
        revision=call('status')['applied_revision']
        job=call('render',output=str(root/'audition.wav'),seconds=.7,format='pcm24')
        done=until(lambda:next((j for j in call('jobs') if j['id']==job['id'] and j['state']!='running'),None))
        assert done['state']=='finished',done
        assert Path(done['result']['Ok']['source'])==source,done
        assert (root/'audition.wav').stat().st_size>1000
        assert call('status')['applied_revision']==revision
        source.write_text(text.replace('C4:q','D4:q'))
        status=until(lambda:s if (s:=call('status'))['applied_revision']>revision and not s['last_error'] else None)
        assert [d['instance_token'] for d in status['devices']]==tokens,'compatible devices were replaced'
        protected=root/'cancel.wav';protected.write_bytes(b'keep me')
        job=call('render',output=str(protected),seconds=3600)
        call('cancel',id=job['id'])
        until(lambda:next((j for j in call('jobs') if j['id']==job['id'] and j['state']=='cancelled'),None))
        assert protected.read_bytes()==b'keep me'
        assert call('status')['render_faults']==0
        call('render',output=str(root/'shutdown.wav'),seconds=3600)
        call('shutdown');process.wait(timeout=10)
        assert process.returncode==0
        assert not (root/'shutdown.wav').exists()
        log.flush()
        diagnostics=(root/'server.log').read_text().lower()
        assert 'wrong thread' not in diagnostics and 'not on the main thread' not in diagnostics,diagnostics
        print(json.dumps({'audio':options.audio,'piano':options.piano,'reload_and_last_good':True,'retained_devices':len(tokens),'independent_bounce':True,'cancel_preserves_output':True,'clean_shutdown':True}))
    except BaseException:
        log.flush()
        print((root/'server.log').read_text())
        raise
    finally:
        if process.poll() is None:process.terminate();process.wait(timeout=10)
        log.close()
