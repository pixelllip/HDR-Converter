import numpy as np, json
from PIL import Image
S=96
patches=[(255,0,0),(0,255,0),(0,0,255),(0,255,255),(255,0,255),(255,255,0),
         (255,128,0),(200,90,60),(128,128,128),(32,32,32),(255,255,255),(255,0,128)]
meta=json.load(open('experiments/gamut_research/cmp_meta.json'))
base=np.asarray(Image.open('experiments/gamut_research/cmp_uhdr.jpg').convert('RGB')).astype(float)
gm=np.asarray(Image.open('experiments/gamut_research/cmp_gainmap.jpg').convert('L')).astype(float)
icc=np.asarray(Image.open('experiments/gamut_research/cmp_icc.png').convert('RGB')).astype(float)
def lin(x):
    v=x/255.0
    return np.where(v<=0.04045, v/12.92, ((v+0.055)/1.055)**2.4)
def pq_inv(c):
    m1=1305/8192;m2=2523/32;c1=107/128;c2=2413/128;c3=2392/128
    p=np.power(np.clip(c,0,1),1/m2)
    return np.power(np.maximum(p-c1,0)/np.maximum(c2-c3*p,1e-12),1/m1)
def sample(a,i,n=3):
    r,q=divmod(i,6)
    blk=a[r*S+40:r*S+56, q*S+40:q*S+56].reshape(-1, n if a.ndim==3 else 1)
    return blk.mean(axis=0)
Y709=np.array([0.2126,0.7152,0.0722]); Y2020=np.array([0.2627,0.6780,0.0593])
print(f'{"patch":>14} | {"ICC Y nits":>11} | {"UHDR Y nits":>12} | {"比":>6}')
ratios=[]
for i,c in enumerate(patches):
    b=sample(base,i); g=sample(gm,i)[0]; p=sample(icc,i)
    lin_b=lin(b)
    log2g=meta['gainMapMinLog2']+(meta['gainMapMaxLog2']-meta['gainMapMinLog2'])*(g/255.0)**meta['gamma']
    hdr=(lin_b+meta['offsetSdr'])*(2**log2g)-meta['offsetHdr']
    y_uhdr=float(np.dot(hdr,Y709))*203
    rel=pq_inv(p/255.0)/(203/10000)   # 输出色域（2020）线性，相对白
    y_icc=float(np.dot(rel,Y2020))*203
    r_=y_uhdr/max(y_icc,1e-6); ratios.append(r_)
    print(f'{str(c):>14} | {y_icc:11.0f} | {y_uhdr:12.0f} | {r_:6.2f}')
print('\nUHDR/ICC 亮度比：中位 %.3f，范围 %.2f–%.2f' % (float(np.median(ratios)), min(ratios), max(ratios)))
