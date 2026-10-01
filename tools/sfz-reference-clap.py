"""Black-box CLAP /2 preset loading and synthetic MIDI rendering; explicit trusted plugin.
Uses public CLAP ABI only; does not inspect or modify plugin binaries/state.
Run under an appropriate GUI/display environment for plugins requiring it.
"""
import ctypes as C,json
import argparse,wave,struct
parser=argparse.ArgumentParser(description=__doc__)
parser.add_argument("--plugin",required=True)
parser.add_argument("--sfz",required=True)
parser.add_argument("--state",help="opaque state saved through public CLAP state API")
parser.add_argument("--save-state",help="save opaque state through public CLAP state API")
parser.add_argument("--output",required=True)
parser.add_argument("--seconds",type=float,default=1)
parser.add_argument("--settle-seconds",type=float,default=0,help="wait after public state load for asynchronous sample preparation")
parser.add_argument("--authored-cc-defaults",action="store_true",help="send root set_cc defaults as rounded 7-bit MIDI; records host setup, not fractional-controller equivalence")
parser.add_argument("--cc",action="append",default=[],help="explicit MIDI controller override N=value")
parser.add_argument("--stereo",action="store_true",help="preserve both output channels for pan/gain calibration")
parser.add_argument("--keyswitch",type=int,action="append",default=[],help="send explicit switch note on/off before the musical note")
parser.add_argument("--key",type=int,default=60)
parser.add_argument("--velocity",type=int,default=100)
parser.add_argument("--note-off",type=float,default=0.625)
parser.add_argument("--gui-ready-file",help="show public embedded GUI and wait for this file before rendering")
args=parser.parse_args()
P=C.c_void_p
class V(C.Structure):_fields_=[('major',C.c_uint32),('minor',C.c_uint32),('revision',C.c_uint32)]
class H(C.Structure):_fields_=[('version',V),('data',P),('name',C.c_char_p),('vendor',C.c_char_p),('url',C.c_char_p),('versionstring',C.c_char_p),('get_extension',P),('restart',P),('process',P),('callback',P)]
class HostPreset(C.Structure):_fields_=[('error',P),('loaded',P)]
error=C.CFUNCTYPE(None,P,C.c_uint32,C.c_char_p,C.c_char_p,C.c_int32,C.c_char_p)(lambda h,k,l,key,e,msg:print(json.dumps({'preset_error':msg.decode() if msg else None,'os_error':e}),flush=True))
loadedcb=C.CFUNCTYPE(None,P,C.c_uint32,C.c_char_p,C.c_char_p)(lambda h,k,l,key:None)
hp=HostPreset(C.cast(error,P),C.cast(loadedcb,P))
import time,select
fds={};timers={}
class HostFD(C.Structure):_fields_=[('register',P),('modify',P),('unregister',P)]
freg=C.CFUNCTYPE(C.c_bool,P,C.c_int,C.c_uint32)(lambda h,fd,flags:(fds.__setitem__(fd,flags) or True))
funreg=C.CFUNCTYPE(C.c_bool,P,C.c_int)(lambda h,fd:(fds.pop(fd,None) is not None))
hf=HostFD(C.cast(freg,P),C.cast(freg,P),C.cast(funreg,P))
class HostTimer(C.Structure):_fields_=[('register',P),('unregister',P)]
def register_timer(h,ms,out):
 i=max(timers,default=0)+1;out[0]=i;timers[i]=[ms*.001,time.monotonic()];return True
tr=C.CFUNCTYPE(C.c_bool,P,C.c_uint32,C.POINTER(C.c_uint32))(register_timer)
tu=C.CFUNCTYPE(C.c_bool,P,C.c_uint32)(lambda h,i:(timers.pop(i,None) is not None))
ht=HostTimer(C.cast(tr,P),C.cast(tu,P))
extensions={b'clap.preset-load/2':hp,b'clap.posix-fd-support':hf,b'clap.timer-support':ht}
get=C.CFUNCTYPE(P,P,C.c_char_p)(lambda h,n:C.addressof(extensions[n]) if n in extensions else None)
request=C.CFUNCTYPE(None,P)(lambda h:None)
h=H(V(1,2,0),None,b'reference',b'muz',b'',b'1',C.cast(get,P),C.cast(request,P),C.cast(request,P),C.cast(request,P))
class E(C.Structure):_fields_=[('version',V),('init',P),('deinit',P),('factory',P)]
class F(C.Structure):_fields_=[('count',P),('descriptor',P),('create',P)]
class D(C.Structure):_fields_=[('version',V),('id',C.c_char_p),('name',C.c_char_p),('vendor',C.c_char_p),('url',C.c_char_p),('manual',C.c_char_p),('support',C.c_char_p),('versionstring',C.c_char_p)]
class Plugin(C.Structure):_fields_=[('descriptor',P),('data',P),('init',P),('destroy',P),('activate',P),('deactivate',P),('start',P),('stop',P),('reset',P),('process',P),('extension',P),('mainthread',P)]
path=args.plugin.encode()
l=C.CDLL(path.decode());e=E.in_dll(l,'clap_entry')
assert C.CFUNCTYPE(C.c_bool,C.c_char_p)(e.init)(path)
f=C.cast(C.CFUNCTYPE(P,C.c_char_p)(e.factory)(b'clap.plugin-factory'),C.POINTER(F)).contents
d=C.cast(C.CFUNCTYPE(P,P,C.c_uint32)(f.descriptor)(C.addressof(f),0),C.POINTER(D)).contents
p=C.CFUNCTYPE(P,P,P,C.c_char_p)(f.create)(C.addressof(f),C.addressof(h),d.id);pl=C.cast(p,C.POINTER(Plugin)).contents
assert C.CFUNCTYPE(C.c_bool,P)(pl.init)(p)
x=C.CFUNCTYPE(P,P,C.c_char_p)(pl.extension)
class Preset(C.Structure):_fields_=[('load',P)]
preset=x(p,b'clap.preset-load/2')
assert preset,'No public CLAP preset-load/2 extension'
loader=C.cast(preset,C.POINTER(Preset)).contents
class State(C.Structure):_fields_=[('save',P),('load',P)]
class Stream(C.Structure):_fields_=[('ctx',P),('fn',P)]
st=C.cast(x(p,b'clap.state'),C.POINTER(State)).contents
loaded=C.CFUNCTYPE(C.c_bool,P,C.c_uint32,C.c_char_p,C.c_char_p)(loader.load)(p,0,args.sfz.encode(),None)
if args.state:
 import io
 saved=io.BytesIO(open(args.state,'rb').read())
 def readstream(stream,buffer,count):
  data=saved.read(count);C.memmove(buffer,data,len(data));return len(data)
 reader=C.CFUNCTYPE(C.c_int64,P,P,C.c_uint64)(readstream)
 stream=Stream(None,C.cast(reader,P))
 loaded=C.CFUNCTYPE(C.c_bool,P,C.POINTER(Stream))(st.load)(p,C.byref(stream))
print(json.dumps({'id':d.id.decode(),'version':d.versionstring.decode(),'public_loader_result':bool(loaded)}),flush=True)
if args.gui_ready_file:
 import os,time
 class Gui(C.Structure):_fields_=[(n,P) for n in ['supported','preferred','create','destroy','scale','size','resize','hints','adjust','setsize','parent','transient','title','show','hide']]
 g=C.cast(x(p,b'clap.gui'),C.POINTER(Gui)).contents
 assert C.CFUNCTYPE(C.c_bool,P,C.c_char_p,C.c_bool)(g.create)(p,b'x11',False)
 xl=C.CDLL('libX11.so.6');xl.XOpenDisplay.argtypes=[C.c_char_p];xl.XOpenDisplay.restype=P
 xd=xl.XOpenDisplay(None);assert xd
 xl.XDefaultRootWindow.argtypes=[P];xl.XDefaultRootWindow.restype=C.c_ulong
 xl.XCreateSimpleWindow.argtypes=[P,C.c_ulong,C.c_int,C.c_int,C.c_uint,C.c_uint,C.c_uint,C.c_ulong,C.c_ulong];xl.XCreateSimpleWindow.restype=C.c_ulong
 win=xl.XCreateSimpleWindow(xd,xl.XDefaultRootWindow(xd),50,50,780,550,0,0,0)
 xl.XMapWindow.argtypes=[P,C.c_ulong];xl.XMapWindow(xd,win)
 xl.XFlush.argtypes=[P];xl.XFlush(xd)
 class Window(C.Structure):_fields_=[('api',C.c_char_p),('handle',C.c_ulong)]
 cw=Window(b'x11',win)
 assert C.CFUNCTYPE(C.c_bool,P,C.POINTER(Window))(g.parent)(p,C.byref(cw))
 assert C.CFUNCTYPE(C.c_bool,P)(g.show)(p)
 glib=C.CDLL('libglib-2.0.so.0');glib.g_main_context_iteration.argtypes=[P,C.c_bool]
 print('GUI ready: load synthetic SFZ through Import, then create ready file',flush=True)
 while not os.path.exists(args.gui_ready_file):
  glib.g_main_context_iteration(None,False)
  fdext=x(p,b'clap.posix-fd-support')
  if fdext and fds:
   fn=C.cast(fdext,C.POINTER(P))[0]
   reads,writes,_=select.select([fd for fd,v in fds.items() if v&1],[fd for fd,v in fds.items() if v&2],[],0)
   for fd in set(reads+writes):C.CFUNCTYPE(None,P,C.c_int,C.c_uint32)(fn)(p,fd,(1 if fd in reads else 0)|(2 if fd in writes else 0))
  te=x(p,b'clap.timer-support')
  if te:
   fn=C.cast(te,C.POINTER(P))[0]
   for i,(period,last) in list(timers.items()):
    if time.monotonic()-last>=period:
     timers[i][1]=time.monotonic();C.CFUNCTYPE(None,P,C.c_uint32)(fn)(p,i)
  if pl.mainthread:C.CFUNCTYPE(None,P)(pl.mainthread)(p)
  time.sleep(.01)
else:
 assert loaded,'Public preset loader rejected SFZ'
if args.save_state:
 data=bytearray()
 def writestream(stream,buffer,count):data.extend(C.string_at(buffer,count));return count
 writer=C.CFUNCTYPE(C.c_int64,P,P,C.c_uint64)(writestream)
 stream=Stream(None,C.cast(writer,P))
 assert C.CFUNCTYPE(C.c_bool,P,C.POINTER(Stream))(st.save)(p,C.byref(stream))
 open(args.save_state,'wb').write(data)
time.sleep(args.settle_seconds)
class Header(C.Structure):_fields_=[('size',C.c_uint32),('time',C.c_uint32),('space',C.c_uint16),('type',C.c_uint16),('flags',C.c_uint32)]
class Midi(C.Structure):_fields_=[('header',Header),('port',C.c_uint16),('data',C.c_uint8*3)]
class Input(C.Structure):_fields_=[('ctx',P),('size',P),('get',P)]
class Output(C.Structure):_fields_=[('ctx',P),('push',P)]
class Buffer(C.Structure):_fields_=[('data32',C.POINTER(C.POINTER(C.c_float))),('data64',P),('channels',C.c_uint32),('latency',C.c_uint32),('constant',C.c_uint64)]
class Process(C.Structure):_fields_=[('steady',C.c_int64),('frames',C.c_uint32),('transport',P),('inputs',P),('outputs',C.POINTER(Buffer)),('input_count',C.c_uint32),('output_count',C.c_uint32),('events',C.POINTER(Input)),('out_events',C.POINTER(Output))]
controller_setup={7:127,11:127,10:64,117:127}
if args.authored_cc_defaults:
 import re
 controller_setup.update({int(n):round(float(v)) for n,v in re.findall(r"set_cc(\d+)\s*=\s*([+-]?[\d.]+)",open(args.sfz).read())})
 controller_setup.update({7:127,11:127,10:64,117:127})
for override in args.cc:
 number,value=map(int,override.split('='));assert 0<=number<128 and 0<=value<128
 controller_setup[number]=value
print(json.dumps({'controller_setup':controller_setup,'settle_seconds':args.settle_seconds}),flush=True)
events=[]
size=C.CFUNCTYPE(C.c_uint32,P)(lambda e:len(events))
getevent=C.CFUNCTYPE(P,P,C.c_uint32)(lambda e,i:C.addressof(events[i]))
push=C.CFUNCTYPE(C.c_bool,P,P)(lambda e,v:True)
inp=Input(None,C.cast(size,P),C.cast(getevent,P));out=Output(None,C.cast(push,P))
a=(C.c_float*48)();b=(C.c_float*48)();channels=(C.POINTER(C.c_float)*2)(a,b);buf=Buffer(channels,None,2,0,0)
assert C.CFUNCTYPE(C.c_bool,P,C.c_double,C.c_uint32,C.c_uint32)(pl.activate)(p,48000,48,48)
assert C.CFUNCTYPE(C.c_bool,P)(pl.start)(p)
process=C.CFUNCTYPE(C.c_int32,P,C.POINTER(Process))(pl.process)
result=[]
for frame in range(0,int(args.seconds*48000)//48*48,48):
 events=[]
 for at,data in [(0,[0xb0,n,v]) for n,v in sorted(controller_setup.items())]+[(0,[0x90,key,100]) for key in args.keyswitch]+[(48,[0x80,key,64]) for key in args.keyswitch]+[(6000,[0x90,args.key,args.velocity]),(int(args.note_off*48000),[0x80,args.key,64])]:
  if frame<=at<frame+48:
   events.append(Midi(Header(C.sizeof(Midi),at-frame,0,10,0),0,(C.c_uint8*3)(*data)))
 a[:]=[0]*48;b[:]=[0]*48
 pr=Process(frame,48,None,None,C.pointer(buf),0,1,C.pointer(inp),C.pointer(out))
 status=process(p,C.byref(pr))
 assert status!=0,('process failed',frame)
 if args.stereo:
  result.extend(value for i in range(48) for value in (float(a[i]),float(b[i])))
 else:
  result.extend((float(a[i])+float(b[i]))*.5 for i in range(48))
with wave.open(args.output,'wb') as w:
 w.setnchannels(2 if args.stereo else 1);w.setsampwidth(2);w.setframerate(48000)
 w.writeframes(b''.join(struct.pack('<h',max(-32768,min(32767,round(v*32767)))) for v in result))
print(json.dumps({'output':args.output,'peak':max(abs(v) for v in result)}),flush=True)
C.CFUNCTYPE(None,P)(pl.stop)(p);C.CFUNCTYPE(None,P)(pl.deactivate)(p)
C.CFUNCTYPE(None,P)(pl.destroy)(p);C.CFUNCTYPE(None)(e.deinit)()
