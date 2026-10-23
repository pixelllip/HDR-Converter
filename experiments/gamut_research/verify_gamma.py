import numpy as np
from PIL import Image
S=96
names=['红','绿','蓝','青','品红','黄','橙','肤色','中灰','暗灰','白','粉']
def pq(c):
    m1=1305/8192;m2=2523/32;c1=107/128;c2=2413/128;c3=2392/128
    p=np.power(np.clip(c,0,1),1/m2)
    return np.power(np.maximum(p-c1,0)/np.maximum(c2-c3*p,1e-12),1/m1)
Y2020=np.array([0.2627,0.6780,0.0593])
for f in ['g1','g08','g12']:
    a=np.asarray(Image.open(f'experiments/gamut_research/{f}.png').convert('RGB')).astype(float)
    vals=[]
    for i in range(12):
        r,q=divmod(i,6)
        blk=a[r*S+40:r*S+56,q*S+40:q*S+56].reshape(-1,3).mean(axis=0)
        y=float(np.dot(pq(blk/255.0)/(203/10000),Y2020))*203
        vals.append(round(y))
    print(f, dict(zip(names,vals)))
