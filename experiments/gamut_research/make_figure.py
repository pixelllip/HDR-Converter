import numpy as np, io
from PIL import Image, ImageDraw, ImageCms
patches=[(255,0,0),(0,255,0),(0,0,255),(0,255,255),(255,0,255),(255,255,0),
         (255,128,0),(200,90,60),(128,128,128),(32,32,32),(255,255,255),(255,0,128)]
S=96; H=56; LBL=300
srgb_prof=ImageCms.createProfile('sRGB')
def center(a,i):
    r,q=divmod(i,6)
    y0=r*S+S//2-H//2; x0=q*S+S//2-H//2
    return a[y0:y0+H, x0:x0+H]
def strip_row(a, normalize=None):
    if a.dtype!=np.uint8:
        v=np.clip(a*(normalize if normalize else 1.0),0,1)
        v=np.where(v<=0.0031308,v*12.92,1.055*np.power(v,1/2.4)-0.055)
        a=(v*255).round().astype(np.uint8)
    row=np.zeros((H,S*len(patches),3),dtype=np.uint8)
    for i in range(len(patches)):
        blk=center(a,i)
        row[:,i*S:(i+1)*S]=np.asarray(Image.fromarray(blk).resize((S,H), Image.NEAREST))
    return row
def rendering(path):
    im=Image.open(path); icc=im.info.get('icc_profile')
    p=ImageCms.ImageCmsProfile(io.BytesIO(icc))
    return np.asarray(ImageCms.profileToProfile(im.convert('RGB'),p,srgb_prof,outputMode='RGB',renderingIntent=1))
def read16(p):
    a=np.asarray(Image.open(p)).astype(np.float64)
    if a.ndim==2: a=np.stack([a]*3,-1)
    a=a/65535.0
    white=center(a,10).mean()
    return a, 1.0/max(white,1e-6)
rows=[('SOURCE (sRGB)', strip_row(np.asarray(Image.open('experiments/gamut_research/patches_srgb.png').convert('RGB'))))]
for g,label in [('srgb','IMAGE out=sRGB (ICC render)'),('p3','IMAGE out=P3 (ICC render)'),('2020','IMAGE out=BT.2020 (ICC render)')]:
    rows.append((label, strip_row(rendering(f'experiments/gamut_research/out_{g}.png'))))
for f,label in [('f_in709_out2020','VIDEO in=709  out=2020'),('f_in2020_out2020','VIDEO in=2020 out=2020'),('f_inp3_outp3','VIDEO in=P3   out=P3')]:
    a,n=read16(f'experiments/gamut_research/{f}.png')
    rows.append((label, strip_row(a,n)))
W=S*len(patches)
img=Image.new('RGB',(LBL+W, H*len(rows)+6*(len(rows)-1)+20), (24,24,28))
d=ImageDraw.Draw(img)
y=10
for label,row in rows:
    d.text((8,y+H//2-6), label, fill=(230,230,235))
    img.paste(Image.fromarray(row),(LBL,y))
    y+=H+6
img.save('experiments/gamut_research/compare.png')
print('saved', img.size)
