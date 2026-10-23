import numpy as np, io
from PIL import Image, ImageCms
def lin_srgb(v):
    v=np.asarray(v,dtype=np.float64)/255.0
    return np.where(v<=0.04045, v/12.92, ((v+0.055)/1.055)**2.4)
base=np.asarray(Image.open('experiments/gamut_research/cmp_uhdr.jpg').convert('RGB'))
icc_im=Image.open('experiments/gamut_research/cmp_icc.png')
prof=ImageCms.ImageCmsProfile(io.BytesIO(icc_im.info['icc_profile']))
srgb=ImageCms.createProfile('sRGB')
rend=np.asarray(ImageCms.profileToProfile(icc_im.convert('RGB'),prof,srgb,outputMode='RGB',renderingIntent=1))
Y=np.array([0.2126,0.7152,0.0722])
def patch(a,i):
    r,q=divmod(i,6); return a[r*96+40, q*96+40]
names=['红','绿','蓝','青','品红','黄','橙','肤色','中灰','暗灰','白','粉']
print(f'{"patch":>4} {"base sRGB":>18} {"ICC 渲染 sRGB":>18}  EV(ICC/base)')
for i,n in enumerate(names):
    b=patch(base,i); r=patch(rend,i)
    ev=np.log2(max(float(lin_srgb(r)@Y),1e-6)/max(float(lin_srgb(b)@Y),1e-6))
    print(f'{n:>4} {str(b):>18} {str(r):>18}  {ev:+.2f}')
