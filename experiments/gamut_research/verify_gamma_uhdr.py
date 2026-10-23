import numpy as np, json
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
icc={'1.0':'g1','0.8':'g08','1.2':'g12'}
print(f'{"gamma":>5} | {"ICC 中灰":>8} {"UHDR 中灰":>9} | {"ICC 白":>7} {"UHDR 白":>8}')
for g in ['1.0','0.8','1.2']:
    base=np.asarray(Image.open(f'experiments/gamut_research/uhdr_g{g}.jpg').convert('RGB')).astype(float)
    gm=np.asarray(Image.open(f'experiments/gamut_research/uhdr_gm_{g}.jpg').convert('L')).astype(float)
    ia=np.asarray(Image.open(f'experiments/gamut_research/{icc[g]}.png').convert('RGB')).astype(float)
    # 用 g1 的元数据做参考？各文件自己的元数据：直接读 XMP 太麻烦，这里用已知公式反推：
    # gainmap 值 0..255 线性映射 log2[min,max]；min=0（log2(1)=0），max 见提取输出
    maxmap={'1.0':1.2483609,'0.8':1.4382,'1.2':0.9789}
    def patch(a,i,n=3):
        r,q=divmod(i,6)
        return a[r*S+40:r*S+56,q*S+40:q*S+56].reshape(-1,n).mean(axis=0)
    out=[]
    for i,name in enumerate(names):
        b=patch(base,i); gmv=patch(gm,i,1)[0]; p=patch(ia,i)
        log2g=maxmap[g]*(gmv/255.0)
        hdr=(lin(b)+1/64)*(2**log2g)-1/64
        y_u=float(np.dot(hdr,Y709))*203
        y_i=float(np.dot(pq(p/255.0)/(203/10000),Y2020))*203
        out.append((name,y_i,y_u))
    print(f'{g:>5} | {out[8][1]:8.0f} {out[8][2]:9.0f} | {out[10][1]:7.0f} {out[10][2]:8.0f}')
    print('      明细:', ', '.join(f'{n} {ui:.0f}/{uu:.0f}' for n,ui,uu in out[:6]))
