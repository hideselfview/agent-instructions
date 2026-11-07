# Schneider's curve fitting: the fewest cubics that stay within a tolerance of a
# run of points, smooth where they join.
import math
sub=lambda a,b:(a[0]-b[0],a[1]-b[1]); add=lambda a,b:(a[0]+b[0],a[1]+b[1]); mul=lambda a,s:(a[0]*s,a[1]*s)
dot=lambda a,b:a[0]*b[0]+a[1]*b[1]
def unit(v):
    n=math.hypot(*v) or 1.0; return (v[0]/n,v[1]/n)
def q(b,t):
    u=1-t; return add(add(mul(b[0],u*u*u),mul(b[1],3*u*u*t)),add(mul(b[2],3*u*t*t),mul(b[3],t*t*t)))
def q1(b,t):
    u=1-t; return add(add(mul(sub(b[1],b[0]),3*u*u),mul(sub(b[2],b[1]),6*u*t)),mul(sub(b[3],b[2]),3*t*t))
def q2(b,t):
    u=1-t; return add(mul(add(sub(b[2],mul(b[1],2)),b[0]),6*u),mul(add(sub(b[3],mul(b[2],2)),b[1]),6*t))
def chord(p):
    u=[0.0]
    for i in range(1,len(p)): u.append(u[-1]+math.hypot(*sub(p[i],p[i-1])))
    return [x/(u[-1] or 1) for x in u]
def gen(p,u,t1,t2):
    a,b=p[0],p[-1]; C=[[0,0],[0,0]]; X=[0,0]
    for i,t in enumerate(u):
        v=1-t; b0=v**3;b1=3*t*v*v;b2=3*t*t*v;b3=t**3
        A1=mul(t1,b1);A2=mul(t2,b2)
        C[0][0]+=dot(A1,A1);C[0][1]+=dot(A1,A2);C[1][1]+=dot(A2,A2)
        tmp=sub(p[i],add(mul(a,b0+b1),mul(b,b2+b3)))
        X[0]+=dot(A1,tmp);X[1]+=dot(A2,tmp)
    C[1][0]=C[0][1]
    d=C[0][0]*C[1][1]-C[0][1]*C[1][0]
    L=math.hypot(*sub(b,a))
    if abs(d)>1e-12:
        al=(X[0]*C[1][1]-X[1]*C[0][1])/d; ar=(C[0][0]*X[1]-C[1][0]*X[0])/d
    else: al=ar=0
    if al<1e-6*L or ar<1e-6*L: al=ar=L/3
    return (a,add(a,mul(t1,al)),add(b,mul(t2,ar)),b)
def err(p,b,u):
    m=0;s=len(p)//2
    for i,t in enumerate(u):
        d=sub(q(b,t),p[i]); e=dot(d,d)
        if e>m: m=e;s=i
    return m,s
def reparam(p,b,u):
    o=[]
    for i,t in enumerate(u):
        d=sub(q(b,t),p[i]);f1=q1(b,t);f2=q2(b,t)
        den=dot(f1,f1)+dot(d,f2)
        o.append(min(1,max(0,t-dot(d,f1)/den)) if abs(den)>1e-12 else t)
    return o
def fit(p,t1,t2,tol):
    if len(p)==2:
        L=math.hypot(*sub(p[1],p[0]))/3
        return [(p[0],add(p[0],mul(t1,L)),add(p[1],mul(t2,L)),p[1])]
    u=chord(p); b=gen(p,u,t1,t2); e,s=err(p,b,u)
    if e<tol*tol: return [b]
    if e<(tol*4)**2:
        for _ in range(6):
            u=reparam(p,b,u); b=gen(p,u,t1,t2); e,s=err(p,b,u)
            if e<tol*tol: return [b]
    s=min(max(s,1),len(p)-2)
    k=min(4,s,len(p)-1-s)
    tc=unit(sub(p[s-k],p[s+k]))
    return fit(p[:s+1],t1,tc,tol)+fit(p[s:],mul(tc,-1),t2,tol)
def end_tan(p,k=4):
    k=min(k,len(p)-1); return unit(sub(p[k],p[0])),unit(sub(p[-1-k],p[-1]))
