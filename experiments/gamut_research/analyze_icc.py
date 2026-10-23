import numpy as np
from PIL import Image, ImageCms
patches=[(255,0,0),(0,255,0),(0,0,255),(0,255,255),(255,0,255),(255,255,0),
         (255,128,0),(200,90,60),(128,128,128),(32,32,32),(255,255,255),(255,0,128)]
S=96
M709=np.array([[0.4123908,0.3575843,0.1804808],[0.2126390,0.7151687,0.0721923],[0.0193308,0.1191948,0.9505322]])
def uvp_lin(v):
    XYZ=M709@v; X,Y,Z=XYZ; d=X+15*Y+3*Z
    return np.array([4*X/d,9*Y/d])
def uvp_srgb(c):
    s=np.array(c)/255.0
    return uvp_lin(np.where(s<=0.04045,s/12.92,((s+0.055)/1.055)**2.4))
srgb_prof=ImageCms.createProfile('sRGB')
for g in ['srgb','p3','2020']:
    p=f'experiments/gamut_research/out_{g}.png'
    im=Image.open(p)
    icc=im.info.get('icc_profile')
    print(g,'icc bytes',len(icc) if icc else None, end=' ')
    if icc:
        src_prof=ImageCms.ImageCmsProfile(__import__('io').BytesIO(icc))
        try:
            conv=ImageCms.profileToProfile(im.convert('RGB'),src_prof,srgb_prof,outputMode='RGB',renderingIntent=1)
            a=np.asarray(conv)
            ds=[]
            for i,c in enumerate(patches):
                r,q=divmod(i,6)
                blk=a[r*S+20:(r+1)*S-20,q*S+20:(q+1)*S-20].reshape(-1,3).mean(axis=0)
                ds.append(float(np.linalg.norm(uvp_srgb(blk)-uvp_srgb(c))))
            print('ICC-rendered dUV:', ' '.join(f'{d:.4f}' for d in ds))
        except Exception as e:
            print('ImageCms failed:',e)
    else:
        print('NO ICC')
