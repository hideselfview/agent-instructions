"""Turn the areas cells.py found into few, clean curves.

Run from the folder cells.py wrote into. Coordinates are in the source PNG's pixels.
"""
import math,sys
import numpy as np
import curvefit as fit
from scipy import ndimage as ni
from skimage import measure,draw as skd
Z=4
F=lambda v:f'{v:.1f}'.rstrip('0').rstrip('.')
lab=np.load('lab.npy'); ink=np.load('ink.npy')   # written by cells.py into the working folder
Dink=ni.distance_transform_edt(ink)
from skimage.morphology import skeletonize
SK=skeletonize(ink)
def cells(*ids): return np.isin(lab,ids)
def grow(m,r): return ni.distance_transform_edt(~m)<=r*Z
def poly(m,pts):
    m=m.copy(); p=np.array(pts,float)*Z
    rr,cc=skd.polygon(p[:,1],p[:,0],m.shape); m[rr,cc]=True; return m
def width(c):
    near=SK&grow(c,8); return float(np.median(Dink[near]))*2/Z
def ring(m,blur=1.5):
    m=ni.binary_fill_holes(m)
    g=ni.gaussian_filter(m.astype(float),blur*Z/2)
    cs=measure.find_contours(g,0.5); c=max(cs,key=len)[:-1]
    return [(x/Z,y/Z) for y,x in c]
def resample(pts,step,closed=True):
    out=[];n=len(pts);carry=0.0
    for i in range(n if closed else n-1):
        (x0,y0),(x1,y1)=pts[i],pts[(i+1)%n]
        L=math.hypot(x1-x0,y1-y0); t=carry
        while t<L: out.append((x0+(x1-x0)*t/L,y0+(y1-y0)*t/L)); t+=step
        carry=t-L
    return out
def turn(p,i,w):
    n=len(p);a,b,c=p[(i-w)%n],p[i],p[(i+w)%n]
    v1=(b[0]-a[0],b[1]-a[1]);v2=(c[0]-b[0],c[1]-b[1])
    return abs(math.degrees(math.atan2(v1[0]*v2[1]-v1[1]*v2[0],v1[0]*v2[0]+v1[1]*v2[1])))
SP=0.5
def corners(p,deg):
    n=len(p);w=int(3/SP);W=int(7/SP)
    t=[turn(p,i,w) for i in range(n)];T=[turn(p,i,W) for i in range(n)]
    cs=[i for i in range(n) if t[i]>=deg and T[i]>=deg*.7 and all(t[i]>=t[(i+k)%n] for k in range(-w,w+1))]
    out=[]
    for i in cs:
        if not out or i-out[-1]>int(5/SP): out.append(i)
    if len(out)>1 and out[0]+n-out[-1]<=int(5/SP): out.pop()
    return out
def sm_open(s,sig):
    n=len(s);r=int(3*sig);o=[s[0]]
    for i in range(1,n-1):
        k=min(r,i,n-1-i)
        if k==0:o.append(s[i]);continue
        w=np.exp(-(np.arange(-k,k+1)**2)/(2*sig*sig));a=np.array(s[i-k:i+k+1])
        o.append(tuple((a*w[:,None]).sum(0)/w.sum()))
    o.append(s[-1]);return o
def sm_ring(p,sig):
    a=np.array(p);return [tuple(v) for v in ni.gaussian_filter1d(a,sig,axis=0,mode='wrap')]
def C(bs): return ''.join(f'C{F(b[1][0])} {F(b[1][1])} {F(b[2][0])} {F(b[2][1])} {F(b[3][0])} {F(b[3][1])}' for b in bs)
def line_or_curves(seg,tol):
    t1,t2=fit.end_tan(seg,6)
    return fit.fit(seg,t1,t2,tol)
def closed(pts,tol=1.2,sig=3,deg=50):
    """A closed outline: real corners stay, everything between them gets the fewest cubics inside tol."""
    p=resample(pts,SP); cs=corners(p,deg); n=len(p)
    if not cs:
        q=sm_ring(p,sig/SP)[::2];h=len(q)//2
        ta=fit.unit(fit.sub(q[3],q[-3]));tb=fit.unit(fit.sub(q[h+3],q[h-3]))
        b=fit.fit(q[:h+1],ta,fit.mul(tb,-1),tol)+fit.fit(q[h:]+[q[0]],tb,fit.mul(ta,-1),tol)
        return f'M{F(q[0][0])} {F(q[0][1])}'+C(b)+'Z',len(b)
    d=f'M{F(p[cs[0]][0])} {F(p[cs[0]][1])}';k=0
    for j,a in enumerate(cs):
        b=cs[(j+1)%len(cs)];L=(b-a)%n or n
        seg=[p[(a+i)%n] for i in range(L+1)]
        seg=sm_open(seg,sig/SP)[::2]+[p[b]]
        c=line_or_curves(seg,tol); d+=C(c);k+=len(c)
    return d+'Z',k
def opened(pts,tol=1.0,sig=2):
    p=resample(pts,SP,False); p=sm_open(p,sig/SP)[::2]+[pts[-1]]
    c=line_or_curves(p,tol); return f'M{F(p[0][0])} {F(p[0][1])}'+C(c),len(c)
