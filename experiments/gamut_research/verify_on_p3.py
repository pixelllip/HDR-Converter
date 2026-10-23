import numpy as np, io
from PIL import Image, ImageCms
patches=[(255,0,0),(0,255,0),(0,0,255),(0,255,255),(255,0,255),(255,255,0)]
S=96
M_P3=np.array([[0.4865709,0.2656677,0.1982173],[0.2289746,0.6917385,0.0792869],[0.0,0.0451134,1.0439444]])
def uv_p3(c):
    s=np.array(c)/255.0
    lin=np.where(s<=0.04045,s/12.92,((s+0.055)/1.055)**2.4)
    XYZ=M_P3@lin; X,Y,Z=XYZ; d=X+15*Y+3*Z
    return np.array([4*X/d,9*Y/d])
W=uv_p3((255,255,255))
p3prof=ImageCms.ImageCmsProfile('assets/display_p3_primary.icc')  # sRGB-TRC P3
for name in ['out_2020_exp0.png','out_2020_exp.png','out_srgb_exp.png']:
    im=Image.open('experiments/gamut_research/'+name)
    prof=ImageCms.ImageCmsProfile(io.BytesIO(im.info['icc_profile']))
    out=np.asarray(ImageCms.profileToProfile(im.convert('RGB'),prof,p3prof,outputMode='RGB',renderingIntent=1))
    sats=[]
    for i in range(6):
        blk=out[20:76,i*S+20:i*S+76].reshape(-1,3).mean(axis=0)
        sats.append(round(float(np.linalg.norm(uv_p3(blk)-W)),3))
    print(name,'-> P3 显示上的彩度:',sats)
print('参考：sRGB 源在 P3 显示 =', [round(float(np.linalg.norm(uv_p3(c)-W)),3) for c in patches])
