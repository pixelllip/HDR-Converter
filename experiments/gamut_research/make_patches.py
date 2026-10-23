import numpy as np
from PIL import Image

# 12 patches, 2 rows x 6 cols, 96px each
patches = [
    (255, 0, 0), (0, 255, 0), (0, 0, 255),
    (0, 255, 255), (255, 0, 255), (255, 255, 0),
    (255, 128, 0), (200, 90, 60), (128, 128, 128),
    (32, 32, 32), (255, 255, 255), (255, 0, 128),
]
S = 96
img = np.zeros((S*2, S*6, 3), dtype=np.uint8)
for i, c in enumerate(patches):
    r, q = divmod(i, 6)
    img[r*S:(r+1)*S, q*S:(q+1)*S] = c
Image.fromarray(img, 'RGB').save(r'experiments\gamut_research\patches_srgb.png')
print('wrote patches_srgb.png', img.shape)
