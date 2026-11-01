import sys
def load(p):
    m={}
    for line in open(p):
        b=line.split()
        if len(b)<14 or b[0]!="F0" or b[6]!="12": continue
        b=[int(x,16) for x in b]
        a=(b[7]<<21)|(b[8]<<14)|(b[9]<<7)|b[10]
        for i,v in enumerate(b[11:-2]): m[a+i]=v
    return m
def fmt(a): return " ".join(f"{(a>>s)&0x7F:02X}" for s in (21,14,7,0))
x,y=load(sys.argv[1]),load(sys.argv[2])
ch=[(a,x.get(a),y.get(a)) for a in sorted(set(x)|set(y)) if x.get(a)!=y.get(a)]
for a,u,v in ch: print(fmt(a), "None" if u is None else f"{u:3d} ({u:02X})", "->", "None" if v is None else f"{v:3d} ({v:02X})")
print(len(ch),"changed;",len(x),"vs",len(y),"bytes")
