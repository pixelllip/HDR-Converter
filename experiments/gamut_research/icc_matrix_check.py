import numpy as np
# ICC D50 PCS matrix from tags
M=np.array([[0.6735,0.1657,0.1251],[0.2790,0.6753,0.0456],[-0.0019,0.0300,0.7972]])
D50=np.array([0.9642,1.0,0.8249]); D65=np.array([0.95047,1.0,1.08883])
# Bradford
B=np.array([[0.8951,0.2664,-0.1614],[-0.7502,1.7135,0.0367],[0.0389,-0.0685,1.0296]])
def adapt(w1,w2):
    s=B@w1; d=B@w2
    return np.linalg.inv(B)@np.diag(d/s)@B
A=adapt(D50,D65)
M709=np.array([[0.4123908,0.3575843,0.1804808],[0.2126390,0.7151687,0.0721923],[0.0193308,0.1191948,0.9505322]])
S=np.linalg.inv(M709)@A@M   # device(2020) RGB -> linear sRGB
print('2020->sRGB matrix:\n',np.round(S,4))
def pq_eotf(x):
    m1=1305/8192;m2=2523/32;c1=107/128;c2=2413/128;c3=2392/128
    p=np.power(np.clip(x,0,1),1/m2)
    return np.power(np.maximum(p-c1,0)/np.maximum(c2-c3*p,1e-12),1/m1)
def srgb_oetf(v):
    v=np.clip(v,0,1)
    return np.where(v<=0.0031308,v*12.92,1.055*np.power(v,1/2.4)-0.055)
patches=[(255,0,0),(0,255,0),(0,0,255),(0,255,255),(255,0,255),(255,255,0),(255,128,0),(200,90,60)]
# encoded 2020 PQ codes measured earlier for each patch
codes={'srgb':[(169,0,0),(0,167,0),(0,0,153),(0,167,167),(172,0,172),(166,166,0),(168,127,0),(153,109,90)],
       '2020':[(156,100,70),(137,165,104),(78,53,150),(140,165,167),(161,107,170),(165,166,107),(158,133,85),(143,115,96)]}
for g in ['srgb','2020']:
    print('==',g,'(matrix-only, no LUT)')
    for c,code in zip(patches,codes[g]):
        pq=np.array(code)/255.0
        lin=pq_eotf(pq)          # 相对 10000nit
        if g=='2020':
            lin_srgb=S@lin
        else:
            lin_srgb=lin
        out=srgb_oetf(lin_srgb)*255
        print(f'  {str(c):>14} -> {np.round(out,0)}')
