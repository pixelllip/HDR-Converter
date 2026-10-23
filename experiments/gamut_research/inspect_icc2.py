import numpy as np, io, struct
from PIL import Image, ImageCms
d=open('assets/2020_profile.icc','rb').read()
n=struct.unpack('>I',d[128:132])[0]
tags={}
for i in range(n):
    b=132+i*12
    tags[d[b:b+4].decode('latin1')]=(struct.unpack('>I',d[b+4:b+8])[0],struct.unpack('>I',d[b+8:b+12])[0])
def s15(o): return struct.unpack('>i',d[o:o+4])[0]/65536.0
for t in ['rXYZ','gXYZ','bXYZ','wtpt']:
    o,s=tags[t]
    print(t,[round(s15(o+8+i*4),4) for i in range(3)])
o,s=tags['cicp']; print('cicp', list(d[o+8:o+12]))
o,s=tags['desc']; print('desc', d[o+8:o+80].split(b'\x00')[0][:60])
# expected D50-adapted BT.2020 primaries (Bradford) from ICC spec
print('expected rXYZ ~', [0.6735,0.2790,-0.0019])

patches=[(255,0,0),(0,255,0),(0,0,255),(0,255,255),(255,0,255),(255,255,0),
         (255,128,0),(200,90,60),(128,128,128),(32,32,32),(255,255,255),(255,0,128)]
S=96
M709=np.array([[0.4123908,0.3575843,0.1804808],[0.2126390,0.7151687,0.0721923],[0.0193308,0.1191948,0.9505322]])
def uvl(v):
    XYZ=M709@v; X,Y,Z=XYZ; dd=X+15*Y+3*Z
    return np.array([4*X/dd,9*Y/dd])
def uvs(c):
    s=np.array(c)/255.0
    return uvl(np.where(s<=0.04045,s/12.92,((s+0.055)/1.055)**2.4))
W=uvl(np.ones(3))
prof=ImageCms.ImageCmsProfile(io.BytesIO(d))
srgb=ImageCms.createProfile('sRGB')
for intent in (0,1):
    im=Image.open('experiments/gamut_research/out_2020.png')
    conv=ImageCms.profileToProfile(im.convert('RGB'),prof,srgb,outputMode='RGB',renderingIntent=intent)
    a=np.asarray(conv)
    print('intent',intent)
    for i,c in enumerate(patches[:8]):
        r,q=divmod(i,6)
        blk=a[r*S+20:(r+1)*S-20,q*S+20:(q+1)*S-20].reshape(-1,3).mean(axis=0)
        sat=float(np.linalg.norm(uvs(blk)-W)); ss=float(np.linalg.norm(uvs(c)-W))
        print(f'  {str(c):>14} rendered={blk.round(1)} sat={sat:.3f} src_sat={ss:.3f} dUV={np.linalg.norm(uvs(blk)-uvs(c)):.4f}')
