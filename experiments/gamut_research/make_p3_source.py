import numpy as np
from PIL import Image, ImageCms
import io
S=96
# Display-P3 编码值：纯 P3 基色（比 sRGB 更艳）+ 混合色
patches=[(255,0,0),(0,255,0),(0,0,255),(255,128,0),(200,90,60),(255,0,128),(128,128,128),(255,255,255)]
img=np.zeros((S,S*len(patches),3),dtype=np.uint8)
for i,c in enumerate(patches):
    img[:,i*S:(i+1)*S]=c
p3icc=open('assets/display_p3_primary.icc','rb').read()
Image.fromarray(img,'RGB').save('experiments/gamut_research/p3_source.png', icc_profile=p3icc)
print('wrote p3_source.png, icc',len(p3icc))
