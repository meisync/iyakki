#!/usr/bin/env python3
"""Reads the Brotli static dictionary, the 121 word transforms and the context look-up table out of the system's libbrotlicommon (F-BROX-19). Writes dict.bin, tr.json and lut.bin in the current folder; crates/iyakki-js/src/modules/brotli/tables.rs is made from tr.json and lut.bin. A build-machine tool, never shipped."""
import ctypes, hashlib, sys
c = ctypes.CDLL('libbrotlicommon.so.1')
class Dict(ctypes.Structure):
    _fields_=[('size_bits',ctypes.c_uint8*32),('offsets',ctypes.c_uint32*32),('data_size',ctypes.c_size_t),('data',ctypes.POINTER(ctypes.c_uint8))]
c.BrotliGetDictionary.restype=ctypes.POINTER(Dict)
d=c.BrotliGetDictionary().contents
print(d.data_size, list(d.size_bits), list(d.offsets))
data=bytes(d.data[:d.data_size])
print(hashlib.sha256(data).hexdigest())
open('dict.bin','wb').write(data)
class Tr(ctypes.Structure):
    _fields_=[('ps_size',ctypes.c_uint16),('ps',ctypes.POINTER(ctypes.c_uint8)),('ps_map',ctypes.POINTER(ctypes.c_uint16)),('n',ctypes.c_uint32),('tr',ctypes.POINTER(ctypes.c_uint8)),('params',ctypes.POINTER(ctypes.c_uint8))]
c.BrotliGetTransforms.restype=ctypes.POINTER(Tr)
t=c.BrotliGetTransforms().contents
print(t.ps_size,t.n)
ps=bytes(t.ps[:t.ps_size])
def s(i):
    off=t.ps_map[i]; ln=ps[off]; return ps[off+1:off+1+ln]
out=[]
for i in range(t.n):
    p,ty,sf=t.tr[3*i],t.tr[3*i+1],t.tr[3*i+2]
    out.append((s(p),ty,s(sf)))
for i,x in enumerate(out[:12]): print(i,x)
import json
json.dump([[list(a),b,list(cc)] for a,b,cc in out],open('tr.json','w'))
lut=ctypes.string_at(ctypes.addressof(ctypes.c_uint8.in_dll(c,'_kBrotliContextLookupTable')),2048)
open('lut.bin','wb').write(lut)
print(list(lut[512+65:512+70]), list(lut[:5]), list(lut[256:260]))
