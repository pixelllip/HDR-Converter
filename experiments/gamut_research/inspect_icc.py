import numpy as np, io, struct
from PIL import Image, ImageCms
p='assets/2020_profile.icc'
d=open(p,'rb').read()
print('size',len(d),'acsp',d[36:40],'cs',d[16:20],'pcs',d[20:24],'class',d[12:16])
n=struct.unpack('>I',d[128:132])[0]
print('tags',n)
tags={}
for i in range(n):
    b=132+i*12
    sig=d[b:b+4].decode('latin1'); off=struct.unpack('>I',d[b+4:b+8])[0]; sz=struct.unpack('>I',d[b+8:b+12])[0]
    tags[sig]=(off,sz)
    print(' ',sig,off,sz)
def s15(o): return struct.unpack('>i',d[o:o+4])[0]/65536.0
for t in ['rXYZ','gXYZ','bXYZ','wtpt','chad']:
    if t in tags:
        o,s=tags[t]
        if t=='chad':
            vals=[s15(o+4+i*4) for i in range(9)]
            print(t,[round(v,4) for v in vals])
        else:
            print(t,[round(s15(o+4+i*4),4) for i in range(3)])
# rTRC / curv
for t in ['rTRC','gTRC','bTRC']:
    if t in tags:
        o,s=tags[t]
        print(t,'type',d[o:o+4], 'count', struct.unpack('>I',d[o+8:o+12])[0] if d[o:o+4]==b'curv' else '-')
