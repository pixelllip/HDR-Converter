import numpy as np
from PIL import Image
patches=[(255,0,0),(0,255,0),(0,0,255),(0,255,255),(255,0,255),(255,255,0),
         (255,128,0),(200,90,60),(128,128,128),(32,32,32),(255,255,255),(255,0,128)]
S=96
M709=np.array([[0.4123908,0.3575843,0.1804808],[0.2126390,0.7151687,0.0721923],[0.0193308,0.1191948,0.9505322]])
MP3=np.array([[0.4865709,0.2656677,0.1982173],[0.2289746,0.6917385,0.0792869],[0.0000000,0.0451134,1.0439444]])
M2020=np.array([[0.6369580,0.1446169,0.1688810],[0.2627002,0.6779981,0.0593017],[0.0000000,0.0280727,1.0609851]])
def uvp_xyz(XYZ):
    X,Y,Z=XYZ; d=X+15*Y+3*Z
    return np.array([4*X/d,9*Y/d])
W=uvp_xyz(M709@np.ones(3))
def pq_eotf(x):
    m1=1305/8192;m2=2523/32;c1=107/128;c2=2413/128;c3=2392/128
    p=np.power(np.clip(x,0,1),1/m2)
    return np.power(np.maximum(p-c1,0)/np.maximum(c2-c3*p,1e-12),1/m1)
def oklab(v):
    r,g,b=v
    l=0.4122214708*r+0.5363325363*g+0.0514459929*b
    m=0.2119034982*r+0.6806995451*g+0.1073969566*b
    s=0.0883024619*r+0.2817188376*g+0.6299787005*b
    l_,m_,s_=np.cbrt(l),np.cbrt(m),np.cbrt(s)
    return np.array([0.2104542553*l_+0.7936177850*m_-0.0040720468*s_,
                     1.9779984951*l_-2.4285922050*m_+0.4505937099*s_,
                     0.0259040371*l_+0.7827717662*m_-0.8086757660*s_])
def measure(path,label,Mt):
    a=np.asarray(Image.open(path).convert('RGB')).astype(float)
    print('==',label)
    for i,c in enumerate(patches):
        r,q=divmod(i,6)
        blk=a[r*S+30:(r+1)*S-30,q*S+30:(q+1)*S-30].reshape(-1,3).mean(axis=0)
        tgt=pq_eotf(blk/255.0)/(203/10000)
        XYZ=Mt@tgt
        sat=float(np.linalg.norm(uvp_xyz(XYZ)-W))
        lab=oklab(tgt)
        hue=float(np.degrees(np.arctan2(lab[2],lab[1])))
        print(f'  {str(c):>14} code={blk.round(0)} sat={sat:.4f} hue={hue:7.1f} tgt={np.round(tgt,3)}')
measure('experiments/gamut_research/out_2020_exp0.png','2020/expand0',M2020)
measure('experiments/gamut_research/out_2020_exp.png','2020/expand1',M2020)
measure('experiments/gamut_research/out_p3_exp.png','p3/expand1',MP3)
print('2020 pure red sat =', np.linalg.norm(uvp_xyz(M2020@np.array([1,0,0]))-W))
print('p3  pure red sat =', np.linalg.norm(uvp_xyz(MP3@np.array([1,0,0]))-W))
