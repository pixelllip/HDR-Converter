import numpy as np
def smoothstep(a,b,x):
    t=np.clip((x-a)/(b-a),0,1); return t*t*(3-2*t)
m=574/203; r=0.45
def target(y):
    t_peak=0.30*(1+(m-1)*0.80)
    lo=y*(1+(m-1)*0.80*smoothstep(0,0.30,y))
    hi=t_peak+((1+(m-1)*r)-t_peak)*smoothstep(0.30,1.0,y)
    return lo if y<=0.30 else hi
print(f"m = peak/white = {m:.3f}   rolloff = {r}")
print(f"{'y(线性)':>8} {'sRGB码':>7} {'目标线性':>9} {'增益':>6} {'HDR nits':>9}")
for y in [0.0,0.02,0.05,0.1,0.18,0.3,0.5,0.7,0.9,1.0]:
    t=target(y); g=(t/y) if y>0 else 1.0
    code = 0 if y<=0.0031308 else 1.055*y**(1/2.4)-0.055
    print(f"{y:8.2f} {code*255:7.0f} {t:9.3f} {g:6.2f} {t*203:9.0f}")
