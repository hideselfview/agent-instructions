#!/usr/bin/env python3
"""Split a flat, outlined drawing into the areas its outlines enclose.

usage: cells.py SOURCE.png [INK_BRIGHTNESS]

Writes into the current folder: rgb.png, ink.npy, lab.npy and cells.png (each
area in its own colour). Prints one line per area, biggest first, and the
outline's width. Raise INK_BRIGHTNESS (default 110) if outlines come out broken.
"""
import subprocess,sys
import numpy as np
import skimage.io as io
from scipy import ndimage as ni
from skimage import measure,morphology

src=sys.argv[1]; limit=float(sys.argv[2]) if len(sys.argv)>2 else 110
subprocess.run(['magick',src,'-background','white','-alpha','remove','-alpha','off','-filter','Mitchell','-resize','400%','rgb.png'],check=True)
subprocess.run(['magick',src,'-alpha','extract','-filter','Mitchell','-resize','400%','alpha.png'],check=True)
rgb=io.imread('rgb.png')[...,:3].astype(float); alpha=io.imread('alpha.png').astype(float)
if alpha.ndim>2: alpha=alpha[...,0]
inside=alpha>128
ink=((.3*rgb[...,0]+.59*rgb[...,1]+.11*rgb[...,2])<limit)&inside
np.save('ink.npy',ink)
widths=ni.distance_transform_edt(ink)[morphology.skeletonize(ink)]*2/4
print('outline width px: median %.1f, p10 %.1f, p90 %.1f'%(np.median(widths),*np.percentile(widths,[10,90])))
lab=measure.label(~ink&inside,connectivity=1); np.save('lab.npy',lab)
for p in sorted((p for p in measure.regionprops(lab) if p.area>200),key=lambda p:-p.area):
    y0,x0,y1,x1=p.bbox
    print(f'{p.label}: area {int(p.area)//16}, box {x0//4} {y0//4} {x1//4} {y1//4}, colour',rgb[lab==p.label].mean(0).round().astype(int).tolist())
colours=np.random.RandomState(3).randint(60,255,(lab.max()+1,3)).astype(np.uint8); colours[0]=[20,20,20]
io.imsave('cells.png',colours[lab][::2,::2],check_contrast=False)
