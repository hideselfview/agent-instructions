#!/usr/bin/env python3
"""Compare a drawing with its source PNG.

usage: compare.py SOURCE.png DRAWING.svg [BACKGROUND]

Stops on invalid XML or repeated ids. Writes sbs.png (source left, drawing
right, 2x), heat.png (real differences in green) and tiles/rRcC.png (a 3x3 grid
of the side-by-side, 3x). Prints the share of real differences and where ink is
missing or extra, biggest first, as (pixels, x, y).

A pixel counts as a real difference only if its colour appears nowhere within
2px in the other image, so anti-aliased edges don't count.
"""
import collections,os,re,subprocess,sys
import numpy as np
import skimage.io as io
from skimage import measure

src,svg=sys.argv[1:3]; bg=sys.argv[3] if len(sys.argv)>3 else '#1e1e1e'
run=lambda *a:subprocess.run(a,check=True)
run('xmllint','--noout',svg)
text=open(svg).read()
dup=[k for k,v in collections.Counter(re.findall(r'\bid="([^"]+)"',text)).items() if v>1]
if dup: sys.exit(f'repeated ids {dup}: browsers drop shapes that librsvg draws')
run('magick',src,'-background',bg,'-flatten','-define','png:color-type=6','PNG32:o1.png')
w=int(subprocess.check_output(['magick','identify','-format','%w','o1.png']))
run('rsvg-convert','-b',bg,'-w',str(w),svg,'-o','t1.png'); run('magick','t1.png','-define','png:color-type=6','PNG32:t1.png')
a=io.imread('o1.png')[...,:3].astype(int); b=io.imread('t1.png')[...,:3].astype(int)
h=min(a.shape[0],b.shape[0]); a,b=a[:h],b[:h]
best=np.full(a.shape[:2],10**6)
for dy in range(-2,3):
    for dx in range(-2,3):
        best=np.minimum(best,np.abs(a-np.roll(b,(dy,dx),(0,1))).sum(2))
real=best>60
print(f'real differences {100*real.mean():.2f}%')
heat=b.astype(np.uint8).copy(); heat[real]=[0,255,0]; io.imsave('heat1.png',heat,check_contrast=False)
run('magick','heat1.png','-filter','point','-resize','200%','heat.png')
lum=lambda r:.3*r[...,0]+.59*r[...,1]+.11*r[...,2]
A,B=lum(a)<80,lum(b)<80
for name,m in (('ink missing',A&~B),('ink extra',B&~A)):
    ps=sorted(measure.regionprops(measure.label(m)),key=lambda p:-p.area)[:9]
    print(name,int(m.sum()),[(int(p.area),int(p.centroid[1]),int(p.centroid[0])) for p in ps])
run('rsvg-convert','-b',bg,'-w',str(w*2),svg,'-o','t2.png'); run('magick','o1.png','-filter','Lanczos','-resize','200%','o2.png')
run('magick','o2.png','t2.png','-background','white','-splice','6x0','+append','sbs.png')
run('rsvg-convert','-b',bg,'-w',str(w*3),svg,'-o','t3.png'); run('magick','o1.png','-filter','Lanczos','-resize','300%','o3.png')
os.makedirs('tiles',exist_ok=True); tw,th=w,h
for r in range(3):
    for c in range(3):
        crop=f'{tw}x{th}+{c*tw}+{r*th}'
        run('magick','(','o3.png','-crop',crop,'+repage',')','(','t3.png','-crop',crop,'+repage',')','-background','white','-splice','6x0','+append',f'tiles/r{r}c{c}.png')
