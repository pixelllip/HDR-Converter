import numpy as np, io
from PIL import Image, ImageDraw, ImageCms
srgb=ImageCms.createProfile('sRGB')
def render(p):
    im=Image.open(p); prof=ImageCms.ImageCmsProfile(io.BytesIO(im.info['icc_profile']))
    return ImageCms.profileToProfile(im.convert('RGB'),prof,srgb,outputMode='RGB',renderingIntent=1)
rows=[('gamma 0.8',render('experiments/gamut_research/gamma_0.8.png')),
      ('gamma 1.0',render('experiments/gamut_research/gamma_1.0.png')),
      ('gamma 1.2',render('experiments/gamut_research/gamma_1.2.png'))]
w,h=rows[0][1].size
H=300; scale=H/h
W=int(w*scale)
img=Image.new('RGB',(W*3+16,H+24),(24,24,28)); d=ImageDraw.Draw(img)
for i,(lab,im) in enumerate(rows):
    img.paste(im.resize((W,H)),(i*(W+8),20))
    d.text((i*(W+8)+4,4),lab,fill=(230,230,235))
img.save('experiments/gamut_research/compare_gamma.png')
print('saved',img.size)
# 数值：平均亮度
for lab,im in rows:
    a=np.asarray(im).astype(float)/255.0
    lin=np.where(a<=0.04045,a/12.92,((a+0.055)/1.055)**2.4)
    print(lab,'mean Y %.4f' % float((lin*np.array([0.2126,0.7152,0.0722])).mean()))
