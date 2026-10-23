import numpy as np
from PIL import Image
patches=[(255,0,0),(0,255,0),(0,0,255),(255,128,0),(200,90,60),(255,0,128),(128,128,128),(255,255,255)]
S=96
M709=np.array([[0.4123908,0.3575843,0.1804808],[0.2126390,0.7151687,0.0721923],[0.0193308,0.1191948,0.9505322]])
MP3=np.array([[0.4865709,0.2656677,0.1982173],[0.2289746,0.6917385,0.0792869],[0.0000000,0.0451134,1.0439444]])
M_709_2020=np.array([[0.6274038959,0.3292830384,0.0433130642],[0.0690972894,0.9195403951,0.0113623156],[0.0163914389,0.0880133078,0.8955952528]])
def uvp_xyz(XYZ):
    X,Y,Z=XYZ; d=X+15*Y+3*Z
    return np.array([4*X/d,9*Y/d])
W=uvp_xyz(M709@np.ones(3))
def pq_eotf(x):
    m1=1305/8192;m2=2523/32;c1=107/128;c2=2413/128;c3=2392/128
    p=np.power(np.clip(x,0,1),1/m2)
    return np.power(np.maximum(p-c1,0)/np.maximum(c2-c3*p,1e-12),1/m1)
def lin(v):
    v=np.clip(v,0,1)
    return np.where(v<=0.04045,v/12.92,((v+0.055)/1.055)**2.4)
a=np.asarray(Image.open('experiments/gamut_research/p3_source_hdr2020.png').convert('RGB')).astype(float)
print(f'{"patch":>14} | {"P3 真实 chroma":>15} | {"管线输出 chroma":>16} | 损失')
for i,c in enumerate(patches):
    blk=a[20:76, i*S+20:i*S+76].reshape(-1,3).mean(axis=0)
    ro=pq_eotf(blk/255.0)/(203/10000)
    lin709=ro@np.linalg.inv(M_709_2020).T      # 输出语义：BT.709 线性（被当作 sRGB 解读的 P3 码值）
    # 参考真值：P3 码值 → P3 线性 → XYZ
    XYZ_p3=MP3@lin(np.array(c)/255.0)
    XYZ_out=M709@np.maximum(lin709,0)
    cp=np.linalg.norm(uvp_xyz(XYZ_p3)-W); co=np.linalg.norm(uvp_xyz(XYZ_out)-W)
    print(f'{str(c):>14} | {cp:15.4f} | {co:16.4f} | {(1-co/cp)*100:5.1f}%')
