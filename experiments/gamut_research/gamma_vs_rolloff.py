import numpy as np
def ss(a,b,x):
    t=np.clip((x-a)/(b-a),0,1); return t*t*(3-2*t)
m=574/203
def target(y,r):
    t_peak=0.30*(1+(m-1)*0.80)
    lo=y*(1+(m-1)*0.80*ss(0,0.30,y))
    hi=t_peak+((1+(m-1)*r)-t_peak)*ss(0.30,1.0,y)
    return lo if y<=0.30 else hi
def t_gamma(y,r,g):
    return target(y**g, r)
ys=[0.02,0.05,0.18,0.30,0.50,1.00]
print('m=%.3f' % m)
print('rolloff 的影响（gamma=1）：')
print('  y     ', ' '.join(f'{y:6.2f}' for y in ys))
for r in (0.0,0.45,1.0):
    print(f'  r={r:4.2f}', ' '.join(f'{target(y,r):6.3f}' for y in ys))
print('\ngamma 的影响（rolloff=0.45）：')
print('  y     ', ' '.join(f'{y:6.2f}' for y in ys))
for g in (0.8,1.0,1.2):
    print(f'  g={g:4.2f}', ' '.join(f'{t_gamma(y,0.45,g):6.3f}' for y in ys))
