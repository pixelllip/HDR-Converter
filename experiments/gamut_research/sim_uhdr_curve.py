import numpy as np
from PIL import Image

def srgb_to_lin(x):
    x=np.asarray(x,dtype=np.float64)/255.0
    return np.where(x<=0.04045, x/12.92, ((x+0.055)/1.055)**2.4)

def smoothstep(a,b,x):
    t=np.clip((x-a)/(b-a),0,1); return t*t*(3-2*t)

def tone_target(y,m,r=0.45):
    # 与 ultra_hdr.rs::tone_target 同式（m<=1 恒等）
    y=np.asarray(y,dtype=np.float64)
    if m<=1.0: return y
    t_peak=0.30*(1+(m-1)*0.80)
    lo=y*(1+(m-1)*0.80*smoothstep(0,0.30,y))
    hi=t_peak+((1+(m-1)*r)-t_peak)*smoothstep(0.30,1.0,y)
    return np.where(y<=0.30, lo, hi)

def tone_gain(y,m,r=0.45):
    y=np.asarray(y,dtype=np.float64)
    out=np.ones_like(y)
    nz=y>0
    out[nz]=tone_target(y[nz],m,r)/y[nz]
    return out

for path in ['tests/nte_bg_0_input.png']:
    im=Image.open(path).convert('RGB')
    im.thumbnail((900,900))
    lin=srgb_to_lin(np.asarray(im))
    y=0.2126*lin[...,0]+0.7152*lin[...,1]+0.0722*lin[...,2]
    exposure=574/203; offset=1/64
    ymax=float(y.max()); m=max(exposure/ymax,1.0) if ymax>0.5 else max(exposure,1.0)
    gt=tone_gain(y,m,0.45)
    g=np.maximum(np.minimum(gt,m),1.0)
    rec=lin*g[...,None]
    print(path, im.size)
    print(f'  m={m:.3f}  gain: min={g.min():.3f} p50={np.median(g):.3f} p95={np.percentile(g,95):.3f} max={g.max():.3f}')
    print(f'  gain<1 像素占比 = {100*np.mean(g<1.0):.2f}%')
    print(f'  gain>1.5 占比 = {100*np.mean(g>1.5):.1f}%   >2.0 = {100*np.mean(g>2.0):.1f}%')
    print(f'  重建后超过 SDR 白(1.0) 的像素 = {100*np.mean(rec.max(axis=-1)>1.0):.1f}%')
    print(f'  超过峰值/白点(2.83) = {100*np.mean(rec.max(axis=-1)>exposure):.2f}%')
    # SDR 呈现（钳到 1.0）后的亮度分布
    def lin_to_srgb(v):
        v=np.clip(v,0,1)
        return np.where(v<=0.0031308, v*12.92, 1.055*v**(1/2.4)-0.055)
    before=(lin_to_srgb(lin.max(axis=-1))*255)
    after=(lin_to_srgb(np.clip(rec,0,1).max(axis=-1))*255)
    print('  最大通道 sRGB 均值: %.1f -> %.1f' % (before.mean(), after.mean()))
    for q in (10,50,90):
        print(f'  p{q}: {np.percentile(before,q):.0f} -> {np.percentile(after,q):.0f}')
    # 保存对照（原图 vs 按规范重建后钳到 SDR）
    out=np.zeros_like(np.asarray(im))
    out[:]=np.clip(lin_to_srgb(np.clip(rec,0,1))*255,0,255).astype(np.uint8)
    Image.fromarray(out).save('experiments/gamut_research/uhdr_reconstruct_sdr.png')
    print('  wrote experiments/gamut_research/uhdr_reconstruct_sdr.png')
