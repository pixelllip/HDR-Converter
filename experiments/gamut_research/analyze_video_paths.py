import numpy as np
from PIL import Image
patches = [(255,0,0),(0,255,0),(0,0,255),(0,255,255),(255,0,255),(255,255,0),
           (255,128,0),(200,90,60),(128,128,128),(32,32,32),(255,255,255),(255,0,128)]
S=96
M709 = np.array([[0.4123908,0.3575843,0.1804808],[0.2126390,0.7151687,0.0721923],[0.0193308,0.1191948,0.9505322]])
def uvp(v):
    XYZ=M709@v; X,Y,Z=XYZ; d=X+15*Y+3*Z
    return np.array([4*X/d,9*Y/d])
W=uvp(np.ones(3))
def read16(p):
    a=np.asarray(Image.open(p)).astype(np.float64)
    if a.ndim==2: a=np.stack([a]*3,-1)
    return a/65535.0
src=np.array([[c/255.0 for c in p] for p in patches])
lin=np.where(src<=0.04045,src/12.92,((src+0.055)/1.055)**2.4)
print(f'{"patch":>14} | {"in709/out2020":>26} | {"in2020/out2020":>26} | {"inp3/outp3":>26}')
for name in ['in709_out2020','in2020_out2020','inp3_outp3']:
    a=read16(f'experiments/gamut_research/f_{name}.png')
    globals()['d_'+name]=a
for i,c in enumerate(patches):
    r,q=divmod(i,6)
    row=f'{str(c):>14}'
    for name in ['in709_out2020','in2020_out2020','inp3_outp3']:
        a=globals()['d_'+name]
        v=a[r*S+30:(r+1)*S-30, q*S+30:(q+1)*S-30].reshape(-1,3).mean(axis=0)*(10000/203.0)
        d=float(np.linalg.norm(uvp(v)-uvp(lin[i])))
        sat=float(np.linalg.norm(uvp(v)-W)); ss=float(np.linalg.norm(uvp(lin[i])-W))
        row+=f' | dUV={d:.4f} sat={sat:.3f}/{ss:.3f}'
    print(row)
