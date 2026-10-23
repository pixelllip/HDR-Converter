import numpy as np
from PIL import Image
patches = [(255,0,0),(0,255,0),(0,0,255),(0,255,255),(255,0,255),(255,255,0),
           (255,128,0),(200,90,60),(128,128,128),(32,32,32),(255,255,255),(255,0,128)]
S=96
M709 = np.array([[0.4123908,0.3575843,0.1804808],[0.2126390,0.7151687,0.0721923],[0.0193308,0.1191948,0.9505322]])
def uvp(XYZ):
    X,Y,Z=XYZ; d=X+15*Y+3*Z
    return np.array([4*X/d,9*Y/d])
W=uvp(M709@np.ones(3))
def chroma(v709):
    return float(np.linalg.norm(uvp(M709@v709)-W))

def read16(path):
    im=Image.open(path)
    a=np.asarray(im).astype(np.float64)
    if a.ndim==2: a=np.stack([a]*3,axis=-1)
    return a/65535.0

src_v=np.array([[c/255.0 for c in p] for p in patches])
srgb_lin=np.where(src_v<=0.04045,src_v/12.92,((src_v+0.055)/1.055)**2.4)

for name in ['frame_src_linear.png','frame_video_2020.png']:
    a=read16('experiments/gamut_research/'+name)
    print('==',name,a.shape,a.max())
    for i,c in enumerate(patches):
        r,q=divmod(i,6)
        blk=a[r*S+30:(r+1)*S-30, q*S+30:(q+1)*S-30].reshape(-1,3).mean(axis=0)
        # src frame: npl=203 scaling → v/1.0 = relative to 203 nit
        v = blk*(10000/203.0) if name.startswith('frame_video') else blk
        print(f'{str(c):>14} v709={np.round(v,3)} dUV={np.linalg.norm(uvp(M709@v)-uvp(M709@srgb_lin[i])):.4f} sat={chroma(v):.3f} (src {chroma(srgb_lin[i]):.3f})')
