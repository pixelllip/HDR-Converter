import numpy as np, io
from PIL import Image, ImageDraw, ImageCms
patches=[(255,0,0),(0,255,0),(0,0,255),(0,255,255),(255,0,255),(255,255,0),
         (255,128,0),(200,90,60),(128,128,128),(32,32,32),(255,255,255),(255,0,128)]
S=96; H=64; LBL=320
srgb=ImageCms.createProfile('sRGB')
def render(p):
    im=Image.open(p); icc=im.info['icc_profile']
    prof=ImageCms.ImageCmsProfile(io.BytesIO(icc))
    return np.asarray(ImageCms.profileToProfile(im.convert('RGB'),prof,srgb,outputMode='RGB',renderingIntent=1))
def strip(a):
    if isinstance(a,np.ndarray) and a.dtype!=np.uint8:
        a=np.asarray(Image.fromarray(a).convert('RGB'))
    row=np.zeros((H,S*12,3),np.uint8)
    for i in range(12):
        r,q=divmod(i,6)
        blk=a[r*S+S//2-H//2:r*S+S//2+H//2, q*S+S//2-H//2:q*S+S//2+H//2]
        row[:,i*S:(i+1)*S]=np.asarray(Image.fromarray(blk).resize((S,H),Image.NEAREST))
    return row
rows=[('SOURCE',strip(np.asarray(Image.open('experiments/gamut_research/patches_srgb.png').convert('RGB')))),
      ('out 2020  expand=0',strip(render('experiments/gamut_research/out_2020_exp0.png'))),
      ('out 2020  expand=1',strip(render('experiments/gamut_research/out_2020_exp.png'))),
      ('out P3    expand=1',strip(render('experiments/gamut_research/out_p3_exp.png')))]
img=Image.new('RGB',(LBL+S*12,H*len(rows)+6*(len(rows)-1)+20),(24,24,28))
d=ImageDraw.Draw(img); y=10
for lab,row in rows:
    d.text((8,y+H//2-6),lab,fill=(230,230,235))
    img.paste(Image.fromarray(row),(LBL,y)); y+=H+6
img.save('experiments/gamut_research/compare_expand.png')
print('saved',img.size)

# 数值：ICC 渲染后的 u'v' 彩度
M709=np.array([[0.4123908,0.3575843,0.1804808],[0.2126390,0.7151687,0.0721923],[0.0193308,0.1191948,0.9505322]])
def uv(c):
    s=np.array(c)/255.0
    lin=np.where(s<=0.04045,s/12.92,((s+0.055)/1.055)**2.4)
    XYZ=M709@lin; X,Y,Z=XYZ; dd=X+15*Y+3*Z
    return np.array([4*X/dd,9*Y/dd])
W=uv((255,255,255))
for name,p in [('expand0','experiments/gamut_research/out_2020_exp0.png'),('expand1','experiments/gamut_research/out_2020_exp.png'),('p3','experiments/gamut_research/out_p3_exp.png')]:
    a=render(p)
    sats=[]
    for i,c in enumerate(patches[:6]):
        r,q=divmod(i,6)
        blk=a[r*S+20:r*S+76,q*S+20:q*S+76].reshape(-1,3).mean(axis=0)
        sats.append(round(float(np.linalg.norm(uv(blk)-W)),3))
    print(name,'rendered chroma (6 色块):',sats)
