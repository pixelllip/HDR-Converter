import numpy as np
from PIL import Image
S=96
names=['红','绿','蓝','青','品红','黄','橙','肤色','中灰','暗灰','白','粉']
def lin(x):
    v=x/255.0
    return np.where(v<=0.04045, v/12.92, ((v+0.055)/1.055)**2.4)
def pq(c):
    m1=1305/8192;m2=2523/32;c1=107/128;c2=2413/128;c3=2392/128
    p=np.power(np.clip(c,0,1),1/m2)
    return np.power(np.maximum(p-c1,0)/np.maximum(c2-c3*p,1e-12),1/m1)
Y709=np.array([0.2126,0.7152,0.0722]); Y2020=np.array([0.2627,0.6780,0.0593])
icc={'1.0':'g10','0.8':'g08','1.2':'g12'}
maxmap={'1.0':1.2483609,'0.8':1.4382,'1.2':0.9789}   # log2(max boost)，由 XMP 提取
for g in ['1.0','0.8','1.2']:
    base=np.asarray(Image.open(f'experiments/gamut_research/uhdr_g{g}.jpg').convert('RGB')).astype(float)
    gmv=np.asarray(Image.open(f'experiments/gamut_research/uhdr_gm_{g}.jpg').convert('L')).astype(float)
    ia=np.asarray(Image.open(f'experiments/gamut_research/{icc[g]}.png').convert('RGB')).astype(float)
    def patch(a,i,n=3):
        r,q=divmod(i,6)
        return a[r*S+40:r*S+56,q*S+40:q*S+56].reshape(-1,n).mean(axis=0)
    print(f'γ={g}')
    for i in [0,7,8,10]:
        b=patch(base,i); gm=patch(gmv,i,1)[0]; p=patch(ia,i)
        hdr=(lin(b)+1/64)*(2**(maxmap[g]*(gm/255.0)))-1/64
        yu=float(np.dot(hdr,Y709))*203
        yi=float(np.dot(pq(p/255.0)/(203/10000),Y2020))*203
        print(f'   {names[i]}: ICC {yi:6.0f}  UHDR {yu:6.0f}  比 {yu/max(yi,1e-6):.2f}')
