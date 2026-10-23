import numpy as np, json
from PIL import Image, ImageCms
from io import BytesIO

patches = [(255,0,0),(0,255,0),(0,0,255),(0,255,255),(255,0,255),(255,255,0),
           (255,128,0),(200,90,60),(128,128,128),(32,32,32),(255,255,255),(255,0,128)]
S=96
def patch_px(path):
    im = Image.open(path).convert('RGB')
    a = np.asarray(im)
    out=[]
    for i,c in enumerate(patches):
        r,q = divmod(i,6)
        blk = a[r*S+20:(r+1)*S-20, q*S+20:(q+1)*S-20].reshape(-1,3)
        out.append(blk.mean(axis=0))
    return np.array(out), im

def pq_eotf(x):
    m1=1305/8192; m2=2523/32; c1=107/128; c2=2413/128; c3=2392/128
    p=np.power(np.clip(x,0,1),1/m2)
    return np.power(np.maximum(p-c1,0)/np.maximum(c2-c3*p,1e-12),1/m1)

M709 = np.array([[0.4123908,0.3575843,0.1804808],
                 [0.2126390,0.7151687,0.0721923],
                 [0.0193308,0.1191948,0.9505322]])
M_709_2020 = np.array([[0.6274038959,0.3292830384,0.0433130642],
                       [0.0690972894,0.9195403951,0.0113623156],
                       [0.0163914389,0.0880133078,0.8955952528]])
M_709_P3 = np.array([[0.822462,0.177538,0.0],
                     [0.033195,0.966805,0.0],
                     [0.017082,0.072397,0.910521]])
M709_inv = np.linalg.inv(M709)

def uvp(XYZ):
    X,Y,Z = XYZ
    d = X+15*Y+3*Z
    return np.array([4*X/d, 9*Y/d])
WHITE = uvp(M709@np.array([1,1,1]))

def chroma(XYZ):
    uv = uvp(XYZ)
    return float(np.linalg.norm(uv-WHITE))

src = np.array(patches,dtype=float)
res = {}
for g,(M, scale) in {
    'srgb':(np.eye(3), 203/10000),
    'p3':(M_709_P3, 203/10000),
    '2020':(M_709_2020, 203/10000),
}.items():
    px,_ = patch_px(f'experiments/gamut_research/out_{g}.png')
    lin_out = pq_eotf(px/255.0)/scale          # 解码回输出色域线性（相对 white=1）
    lin709 = lin_out @ np.linalg.inv(M).T       # 输出色域 → BT.709 线性
    rows=[]
    for i in range(len(patches)):
        v = np.maximum(lin709[i],0)
        XYZ = M709@v
        src_v = np.array(patches[i],dtype=float)/255.0
        srgb_lin = np.where(src_v<=0.04045, src_v/12.92, ((src_v+0.055)/1.055)**2.4)
        XYZ_src = M709@srgb_lin
        rows.append(dict(code=px[i].round(1).tolist(),
                         lin709=[round(float(x),4) for x in lin709[i]],
                         uv=[round(float(x),4) for x in uvp(XYZ)],
                         uv_src=[round(float(x),4) for x in uvp(XYZ_src)],
                         duv_from_src=round(float(np.linalg.norm(uvp(XYZ)-uvp(XYZ_src))),4),
                         sat=round(chroma(XYZ),4), sat_src=round(chroma(XYZ_src),4)))
    res[g]=rows

print('patch      ' + ' | '.join(f'{g:>34}' for g in res))
for i,c in enumerate(patches):
    line = f'{str(c):>14}'
    for g in res:
        r=res[g][i]
        line += f' | code={r["code"]!s:>18} dUV={r["duv_from_src"]:.4f} sat={r["sat"]:.3f}/{r["sat_src"]:.3f}'
    print(line)
json.dump(res, open('experiments/gamut_research/measured.json','w'), indent=1)
