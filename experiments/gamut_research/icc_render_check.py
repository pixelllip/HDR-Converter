import numpy as np, io
from PIL import Image, ImageCms
prof_srgb=ImageCms.createProfile('sRGB')
for g in ['srgb','p3','2020']:
    im=Image.open(f'experiments/gamut_research/out_{g}.png').convert('RGB')
    icc=Image.open(f'experiments/gamut_research/out_{g}.png').info['icc_profile']
    p=ImageCms.ImageCmsProfile(io.BytesIO(icc))
    out=ImageCms.profileToProfile(im,p,prof_srgb,outputMode='RGB',renderingIntent=1)
    a=np.asarray(out)
    print(g,'icc',len(icc),'red patch rendered',a[40,40],'gray patch',a[40,40+8*96] if a.shape[1]>800 else None)
    # profile description
    print('   desc:',ImageCms.getProfileDescription(p).strip(),'| version', ImageCms.getProfileInfo(p)[:40].replace('\n',' '))
