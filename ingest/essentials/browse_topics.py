"""Essentials v2: browse sections for essentials-v2-wikipedia-en.tsv (sp browse --sections).

  cd <work dir> && python essentials/browse_topics.py      # writes browse-sections-v2.tsv (title, section)

Articles from beyond v0's candidates have no topic; each gets the topic of its 15 nearest topic-labelled articles
in the list (lead embeddings from shard-leads/, cosine^4-weighted vote).
"""
import csv, sys, collections, glob, numpy as np
csv.field_size_limit(sys.maxsize)
NAME={'society':'Society, law & history','radio':'Radio & communications','water':'Water','tools':'Tools & materials',
 'shelter':'Shelter & building','food':'Food & cooking','navigation':'Navigation & places','hazards':'Hazards & disasters',
 'foraging':'Plants, animals & foraging','chemistry':'Chemistry','energy':'Energy & power','science':'Science',
 'agriculture':'Farming','medicine':'Health & medicine','general':'General knowledge'}
rows=list(csv.DictReader(open('essentials-v2-wikipedia-en.tsv'),delimiter='\t'))
want={int(r['page_id']) for r in rows}
ids=[];E=[]
for f in sorted(glob.glob('shard-leads/*.npz')):
    z=np.load(f); k=np.isin(z['ids'],list(want))
    ids.append(z['ids'][k]); E.append(z['emb'][k].astype(np.float32))
ids=np.concatenate(ids); E=np.concatenate(E)
ids,first=np.unique(ids,return_index=True); E=E[first]
pos={int(p):i for i,p in enumerate(ids)}
lab=[(pos[int(r['page_id'])],r['topic']) for r in rows if r['topic'] and int(r['page_id']) in pos]
li=np.array([i for i,_ in lab]); lt=np.array([t for _,t in lab])
L=E[li]
todo=[r for r in rows if not r['topic']]
got=collections.Counter(); out={}
for s in range(0,len(todo),2000):
    part=[r for r in todo[s:s+2000] if int(r['page_id']) in pos]
    Q=E[[pos[int(r['page_id'])] for r in part]]
    S=Q@L.T; nn=np.argpartition(-S,15,axis=1)[:,:15]
    for r,row,sims in zip(part,nn,S[np.arange(len(part))[:,None],nn]):
        c=collections.Counter()
        for j,w in zip(row,sims): c[lt[j]]+=w**4
        t=c.most_common(1)[0][0]; out[r['title']]=t; got[t]+=1
print(len(todo),'without topic,',len(out),'assigned:',got.most_common())
with open('browse-sections-v2.tsv','w') as f:
    for r in rows:
        t=r['topic'] or out.get(r['title'],'general')
        f.write(f"{r['title']}\t{NAME[t]}\n")
import random; random.seed(3)
print('sample:', [(t,out[t]) for t in random.sample(sorted(out),15)])
